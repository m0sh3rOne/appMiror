//! Moteur séquentiel asynchrone des graphes de workflow.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::app::AttachedWindow;
use crate::protocol::*;
use crate::{capture, input};

#[derive(Debug, Clone)]
pub enum WorkflowEvent {
    Started { execution_id: String, name: String },
    NodeStarted { execution_id: String, node_id: String },
    NodeFinished { execution_id: String, node_id: String, output: WorkflowHandle },
    Finished { execution_id: String, name: String, nodes_executed: usize },
    Failed { execution_id: String, node_id: Option<String>, message: String },
}

fn emit(tx: &Option<mpsc::UnboundedSender<WorkflowEvent>>, event: WorkflowEvent) {
    if let Some(tx) = tx { let _ = tx.send(event); }
}

fn resolve_window_context(window_id: isize) -> String {
    capture::list_windows(&[], true)
        .ok()
        .and_then(|wins| wins.into_iter().find(|w| w.id == window_id))
        .map(|w| w.context)
        .unwrap_or_default()
}

async fn eval_condition(expression: &str, last_success: bool, target: Option<&AttachedWindow>) -> Result<bool, String> {
    let trimmed = expression.trim();
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "true" | "always" => Ok(true),
        "false" | "never" => Ok(false),
        "last_result == \"success\"" | "last_result == 'success'" => Ok(last_success),
        "last_result == \"error\"" | "last_result == 'error'" => Ok(!last_success),
        _ => {
            if lower.starts_with("state ==") || lower.starts_with("state !=") {
                let is_eq = lower.starts_with("state ==");
                let expected_part = if is_eq { &trimmed[8..] } else { &trimmed[8..] }.trim();
                let expected_name = expected_part.trim_matches(|c| c == '"' || c == '\'');
                let Some(target) = target else { return Err("condition d'état sans fenêtre cible active".into()); };
                let context = resolve_window_context(target.id);
                let states = crate::state_store::load_context(&context).await.unwrap_or_default();
                let (session, rx) = crate::capture::start(target.id, 80, 1.0, false)
                    .map_err(|e| format!("capture de vérification pour condition: {e}"))?;
                tokio::time::sleep(Duration::from_millis(200)).await;
                let frame = rx.borrow().clone();
                let _ = session.stop();
                let detected_state = if !frame.is_empty() {
                    crate::detect::detect(&frame, &states).ok().and_then(|(b, _)| b).map(|s| s.name)
                } else {
                    None
                };
                let matched = detected_state.as_deref().is_some_and(|name| name.eq_ignore_ascii_case(expected_name));
                Ok(if is_eq { matched } else { !matched })
            } else {
                Err(format!("condition non supportée: {trimmed}"))
            }
        }
    }
}

fn next_node(graph: &WorkflowGraph, source: &str, handle: WorkflowHandle) -> Option<String> {
    graph.edges.iter().find(|e| e.source_node_id == source && e.source_handle == handle).map(|e| e.target_node_id.clone())
}

pub async fn run(
    file: WorkflowFile,
    fallback_target: Option<AttachedWindow>,
    events: Option<mpsc::UnboundedSender<WorkflowEvent>>,
) -> Result<usize, String> {
    run_full(file, fallback_target, events, None, None, None).await
}

pub async fn run_full(
    file: WorkflowFile,
    fallback_target: Option<AttachedWindow>,
    events: Option<mpsc::UnboundedSender<WorkflowEvent>>,
    app_state: Option<crate::app::AppState>,
    cancel_rx: Option<tokio::sync::watch::Receiver<bool>>,
    execution_id_opt: Option<String>,
) -> Result<usize, String> {
    crate::workflow_store::validate(&file.graph)?;
    let execution_id = execution_id_opt.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let (internal_cancel_tx, internal_cancel_rx) = tokio::sync::watch::channel(false);
    let cancel_rx = cancel_rx.unwrap_or(internal_cancel_rx);

    let start_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    if let Some(ref state) = app_state {
        let record = ExecutionRecord {
            execution_id: execution_id.clone(),
            workflow_name: file.name.clone(),
            status: ExecutionStatus::Running,
            current_node_id: None,
            started_at_ms: start_ts,
            finished_at_ms: None,
            nodes_executed: 0,
            history: Vec::new(),
            error_message: None,
        };
        state.record_execution(record, internal_cancel_tx);
    }

    emit(&events, WorkflowEvent::Started { execution_id: execution_id.clone(), name: file.name.clone() });
    let start = file.graph.nodes.values().find(|n| n.node_type == WorkflowNodeType::Start).unwrap().id.clone();
    let mut current = start;
    let mut loops: HashMap<String, u32> = HashMap::new();
    let mut last_success = true;
    let mut executed = 0usize;
    let mut active_target = fallback_target;
    let mut force_foreground = false;

    let result: Result<usize, (Option<String>, String)> = async {
        loop {
            if *cancel_rx.borrow() {
                if let Some(ref state) = app_state {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
                    state.update_execution(&execution_id, |rec| {
                        rec.status = ExecutionStatus::Cancelled;
                        rec.finished_at_ms = Some(now);
                        rec.error_message = Some("workflow annulé".into());
                    });
                }
                return Err((Some(current.clone()), "workflow annulé".into()));
            }
            if executed >= 10_000 { return Err((Some(current.clone()), "limite de 10 000 transitions atteinte".into())); }
            let node = file.graph.nodes.get(&current).ok_or_else(|| (Some(current.clone()), "nœud introuvable".into()))?;

            if let Some(ref state) = app_state {
                state.update_execution(&execution_id, |rec| {
                    rec.current_node_id = Some(current.clone());
                });
            }

            emit(&events, WorkflowEvent::NodeStarted { execution_id: execution_id.clone(), node_id: current.clone() });
            executed += 1;
            let node_start_ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;

            let output = match &node.data {
                WorkflowNodeData::Start(data) => {
                    let target = resolve_start_target(data, active_target).await.map_err(|e| (Some(node.id.clone()), e))?;
                    let context = resolve_window_context(target.id);
                    let profile = crate::profile_store::load_or_default(&context).await;
                    force_foreground = profile.foreground_required
                        || (data.focus
                            && data.execution_mode == AppExecutionMode::MainDesktop
                            && !matches!(effective_target_kind(data), StartTargetKind::MainDesktop | StartTargetKind::VirtualDesktop));
                    active_target = Some(target);
                    WorkflowHandle::Next
                }
                WorkflowNodeData::Macro(data) => {
                    let target = active_target.ok_or_else(|| (Some(node.id.clone()), "aucune application cible; configurez le nœud START".into()))?;
                    let macro_file = match &data.macro_context {
                        Some(context) => crate::macro_store::load_named(context, &data.macro_name).await,
                        None => crate::macro_store::load_by_name(&data.macro_name).await,
                    };
                    let injection_mode = if force_foreground { InjectionMode::Foreground } else { data.injection_mode };
                    if injection_mode == InjectionMode::Foreground {
                        input::focus_window(target.id);
                        tokio::time::sleep(Duration::from_millis(180)).await;
                    }
                    match macro_file {
                        Ok(m) => match crate::macro_player::play_full(&m, target, injection_mode, &data.parameters, 0).await {
                            Ok(_) => { last_success = true; WorkflowHandle::OnSuccess }
                            Err(e) => { tracing::warn!("macro {} échouée: {e}", data.macro_name); last_success = false; WorkflowHandle::OnError }
                        },
                        Err(e) => { tracing::warn!("macro {} indisponible: {e}", data.macro_name); last_success = false; WorkflowHandle::OnError }
                    }
                }
                WorkflowNodeData::Activate(data) => {
                    let target = active_target.ok_or_else(|| (Some(node.id.clone()), "aucune application cible; configurez le nœud START".into()))?;
                    let context = resolve_window_context(target.id);
                    let layouts = crate::layout_store::load_context(&context).await.unwrap_or_default();
                    let states = crate::state_store::load_context(&context).await.unwrap_or_default();
                    let strategy = crate::activate::Strategy::parse(data.strategy.as_deref())
                        .map_err(|e| (Some(node.id.clone()), e))?;
                    let retries = data.retries.min(10);
                    let mut success = false;
                    for attempt in 0..=retries {
                        if *cancel_rx.borrow() { break; }
                        if attempt > 0 {
                            let delay = Duration::from_millis(300 + u64::from(attempt) * 200);
                            tokio::time::sleep(delay).await;
                        }
                        match crate::activate::activate_element(
                            target,
                            &layouts,
                            &states,
                            data.layout.as_deref(),
                            &data.element,
                            strategy,
                            data.verify,
                            None,
                            data.verification,
                        ).await {
                            Ok(_) => {
                                success = true;
                                break;
                            }
                            Err(e) => {
                                tracing::warn!("activate {} échoué (tentative {}/{}): {e}", data.element, attempt + 1, retries + 1);
                            }
                        }
                    }
                    if success {
                        last_success = true;
                        WorkflowHandle::OnSuccess
                    } else {
                        last_success = false;
                        WorkflowHandle::OnError
                    }
                }
                WorkflowNodeData::WaitForState(data) => {
                    let target = active_target.ok_or_else(|| (Some(node.id.clone()), "aucune application cible; configurez le nœud START".into()))?;
                    let context = resolve_window_context(target.id);
                    let states = crate::state_store::load_context(&context).await.unwrap_or_default();
                    match crate::detect::wait_for_state_on_window(
                        target.id,
                        &states,
                        &data.state,
                        data.timeout_ms,
                    ).await {
                        Ok(_) => {
                            last_success = true;
                            WorkflowHandle::OnSuccess
                        }
                        Err(e) => {
                            tracing::warn!("wait_for_state {} échoué: {e}", data.state);
                            last_success = false;
                            WorkflowHandle::OnError
                        }
                    }
                }
                WorkflowNodeData::Loop(data) => {
                    let counter = loops.entry(node.id.clone()).or_insert(0);
                    if *counter < data.repetitions { *counter += 1; WorkflowHandle::LoopCycle }
                    else { loops.remove(&node.id); WorkflowHandle::Out }
                }
                WorkflowNodeData::Condition(data) => {
                    if eval_condition(&data.expression, last_success, active_target.as_ref()).await.map_err(|e| (Some(node.id.clone()), e))? {
                        WorkflowHandle::True
                    } else {
                        WorkflowHandle::False
                    }
                }
                WorkflowNodeData::Wait(data) => {
                    tokio::time::sleep(Duration::from_millis(data.duration_ms)).await;
                    WorkflowHandle::WaitDone
                }
            };

            let node_finish_ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
            let duration_ms = node_finish_ts.saturating_sub(node_start_ts);

            let screenshot_b64 = if output == WorkflowHandle::OnError {
                if let Some(target) = active_target {
                    if let Ok((_sess, rx)) = capture::start(target.id, 80, 1.0, false) {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let bytes = rx.borrow().clone();
                        if !bytes.is_empty() {
                            Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(ref state) = app_state {
                let log_entry = NodeExecutionLog {
                    node_id: current.clone(),
                    node_type: format!("{:?}", node.node_type).to_lowercase(),
                    started_at_ms: node_start_ts,
                    finished_at_ms: Some(node_finish_ts),
                    duration_ms: Some(duration_ms),
                    output_handle: Some(format!("{:?}", output).to_lowercase()),
                    action_attempted: Some(format!("{:?}", node.data)),
                    state_before: None,
                    state_after: None,
                    attempts_count: Some(1),
                    selection_reason: if force_foreground { Some("profile foreground_required=true => SendInput forced".into()) } else { None },
                    error_message: if output == WorkflowHandle::OnError { Some("étape échouée".into()) } else { None },
                    screenshot_base64: screenshot_b64,
                };
                state.update_execution(&execution_id, |rec| {
                    rec.nodes_executed = executed;
                    rec.history.push(log_entry);
                });
            }

            emit(&events, WorkflowEvent::NodeFinished { execution_id: execution_id.clone(), node_id: node.id.clone(), output });
            tokio::time::sleep(Duration::from_millis(50)).await;
            let Some(next) = next_node(&file.graph, &node.id, output) else {
                if output == WorkflowHandle::OnError { return Err((Some(node.id.clone()), "étape échouée sans branche on_error".into())); }
                return Ok(executed);
            };
            current = next;
        }
    }.await;

    let finish_ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    match result {
        Ok(count) => {
            if let Some(ref state) = app_state {
                state.update_execution(&execution_id, |rec| {
                    rec.status = ExecutionStatus::Finished;
                    rec.current_node_id = None;
                    rec.finished_at_ms = Some(finish_ts);
                });
            }
            emit(&events, WorkflowEvent::Finished { execution_id, name: file.name, nodes_executed: count });
            Ok(count)
        }
        Err((node_id, message)) => {
            if let Some(ref state) = app_state {
                state.update_execution(&execution_id, |rec| {
                    if rec.status != ExecutionStatus::Cancelled {
                        rec.status = ExecutionStatus::Failed;
                    }
                    rec.finished_at_ms = Some(finish_ts);
                    rec.error_message = Some(message.clone());
                });
            }
            emit(&events, WorkflowEvent::Failed { execution_id, node_id, message: message.clone() });
            Err(message)
        }
    }
}

fn effective_target_kind(data: &StartNodeData) -> StartTargetKind {
    if let Some(kind) = data.target_kind { return kind; }
    // Compatibilité avec les premiers workflows : lorsque le contexte n'a pas
    // pu être résolu (`unknown`) mais qu'une commande est présente, la commande
    // doit primer sur un ancien titre d'application resté dans le JSON.
    let context = data.app_context.as_deref().unwrap_or("").trim();
    if data.command.as_deref().is_some_and(|v| !v.trim().is_empty())
        && (context.is_empty() || context.eq_ignore_ascii_case("unknown")) {
        StartTargetKind::Command
    } else {
        StartTargetKind::Window
    }
}

fn command_context(data: &StartNodeData) -> String {
    let command = data.command.as_deref().unwrap_or("").trim().trim_matches('"');
    std::path::Path::new(command)
        .file_name()
        .map(|v| v.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn find_target(data: &StartNodeData, kind: StartTargetKind) -> Result<Option<AttachedWindow>, String> {
    let mut context = match kind {
        StartTargetKind::Command => command_context(data),
        _ => data.app_context.as_deref().unwrap_or("").trim().to_lowercase(),
    };
    if context == "unknown" { context.clear(); }
    let title = if kind == StartTargetKind::Window {
        data.app_title.as_deref().unwrap_or("").trim().to_lowercase()
    } else {
        String::new()
    };
    let title_suffix = title.rsplit_once(" — ").or_else(|| title.rsplit_once(" - ")).map(|(_, suffix)| suffix.trim()).unwrap_or(&title);
    if context.is_empty() && title.is_empty() { return Ok(None); }
    let windows = capture::list_windows(&[], true).map_err(|e| format!("liste des applications: {e}"))?;
    let found = windows.into_iter().find(|w| {
        if kind == StartTargetKind::MainDesktop { return w.is_desktop && !w.is_virtual; }
        if kind == StartTargetKind::VirtualDesktop { return w.is_desktop && w.is_virtual; }
        if w.is_desktop { return false; }
        #[cfg(windows)]
        let correct_desktop = match data.execution_mode {
            AppExecutionMode::MainDesktop => !w.is_virtual,
            AppExecutionMode::VirtualDesktop => w.is_virtual,
        };
        #[cfg(not(windows))]
        let correct_desktop = true;
        let window_title = w.title.to_lowercase();
        correct_desktop && ((!context.is_empty() && w.context.eq_ignore_ascii_case(&context))
            || (!title.is_empty() && (window_title.contains(&title) || (!title_suffix.is_empty() && window_title.contains(title_suffix)))))
    });
    Ok(found.and_then(|w| input::client_size(w.id).map(|(width, height)| AttachedWindow { id: w.id, width, height })))
}

fn launch_start_app(data: &StartNodeData) -> Result<(), String> {
    let command = data.command.as_deref().map(str::trim).filter(|s| !s.is_empty())
        .or_else(|| data.app_context.as_deref().map(str::trim).filter(|s| !s.is_empty()))
        .ok_or_else(|| "application absente et aucune commande de lancement configurée".to_string())?;
    match data.execution_mode {
        AppExecutionMode::MainDesktop => {
            std::process::Command::new(command).spawn().map_err(|e| format!("lancement de {command}: {e}"))?;
        }
        AppExecutionMode::VirtualDesktop => {
            #[cfg(windows)]
            {
                let desktop = crate::input::desktop::get_or_create_virtual_desktop().ok_or_else(|| "bureau virtuel indisponible".to_string())?;
                let args: Vec<String> = Vec::new();
                crate::input::desktop::spawn_on_desktop(command, &args, "WinSta0\\appMirror_Desktop").map_err(|e| format!("lancement virtuel de {command}: {e}"))?;
                let _ = desktop;
            }
            #[cfg(not(windows))]
            {
                std::process::Command::new(command).spawn().map_err(|e| format!("lancement de {command}: {e}"))?;
            }
        }
    }
    Ok(())
}

async fn resolve_start_target(data: &StartNodeData, fallback: Option<AttachedWindow>) -> Result<AttachedWindow, String> {
    let kind = effective_target_kind(data);
    let is_desktop = matches!(kind, StartTargetKind::MainDesktop | StartTargetKind::VirtualDesktop);
    let has_selector = is_desktop
        || kind == StartTargetKind::Command
        || data.app_context.as_deref().is_some_and(|v| !v.trim().is_empty() && !v.eq_ignore_ascii_case("unknown"))
        || data.app_title.as_deref().is_some_and(|v| !v.trim().is_empty());
    let mut target = if has_selector { find_target(data, kind)? } else { fallback };
    if target.is_none() && !is_desktop && (has_selector || kind == StartTargetKind::Command) {
        launch_start_app(data)?;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Some(found) = find_target(data, kind)? { target = Some(found); break; }
        }
    }
    let target = target.ok_or_else(|| "cible introuvable après lancement".to_string())?;
    if data.focus && !is_desktop && data.execution_mode == AppExecutionMode::MainDesktop {
        // Plusieurs tentatives laissent le temps aux fenêtres WinUI de finir
        // leur initialisation avant l'injection de la première touche.
        for _ in 0..6 {
            input::focus_window(target.id);
            tokio::time::sleep(Duration::from_millis(120)).await;
            if input::is_foreground_window(target.id) { break; }
        }
    }
    Ok(target)
}
