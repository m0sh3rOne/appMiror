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

fn condition(expression: &str, last_success: bool) -> Result<bool, String> {
    match expression.trim().to_ascii_lowercase().as_str() {
        "true" | "always" => Ok(true),
        "false" | "never" => Ok(false),
        "last_result == \"success\"" | "last_result == 'success'" => Ok(last_success),
        "last_result == \"error\"" | "last_result == 'error'" => Ok(!last_success),
        other => Err(format!("condition non supportée: {other}")),
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
    crate::workflow_store::validate(&file.graph)?;
    let execution_id = uuid::Uuid::new_v4().to_string();
    emit(&events, WorkflowEvent::Started { execution_id: execution_id.clone(), name: file.name.clone() });
    let start = file.graph.nodes.values().find(|n| n.node_type == WorkflowNodeType::Start).unwrap().id.clone();
    let mut current = start;
    let mut loops: HashMap<String, u32> = HashMap::new();
    let mut last_success = true;
    let mut executed = 0usize;
    let mut active_target = fallback_target;

    let result: Result<usize, (Option<String>, String)> = async {
        loop {
            if executed >= 10_000 { return Err((Some(current.clone()), "limite de 10 000 transitions atteinte".into())); }
            let node = file.graph.nodes.get(&current).ok_or_else(|| (Some(current.clone()), "nœud introuvable".into()))?;
            emit(&events, WorkflowEvent::NodeStarted { execution_id: execution_id.clone(), node_id: current.clone() });
            executed += 1;
            let output = match &node.data {
                WorkflowNodeData::Start(data) => {
                    active_target = Some(resolve_start_target(data, active_target).await.map_err(|e| (Some(node.id.clone()), e))?);
                    WorkflowHandle::Next
                }
                WorkflowNodeData::Macro(data) => {
                    let target = active_target.ok_or_else(|| (Some(node.id.clone()), "aucune application cible; configurez le nœud START".into()))?;
                    let macro_file = match &data.macro_context {
                        Some(context) => crate::macro_store::load_named(context, &data.macro_name).await,
                        None => crate::macro_store::load_by_name(&data.macro_name).await,
                    };
                    match macro_file {
                        Ok(m) => match crate::macro_player::play_with_mode(&m, target, data.injection_mode).await {
                            Ok(_) => { last_success = true; WorkflowHandle::OnSuccess }
                            Err(e) => { tracing::warn!("macro {} échouée: {e}", data.macro_name); last_success = false; WorkflowHandle::OnError }
                        },
                        Err(e) => { tracing::warn!("macro {} indisponible: {e}", data.macro_name); last_success = false; WorkflowHandle::OnError }
                    }
                }
                WorkflowNodeData::Loop(data) => {
                    let counter = loops.entry(node.id.clone()).or_insert(0);
                    if *counter < data.repetitions { *counter += 1; WorkflowHandle::LoopCycle }
                    else { loops.remove(&node.id); WorkflowHandle::Out }
                }
                WorkflowNodeData::Condition(data) => {
                    if condition(&data.expression, last_success).map_err(|e| (Some(node.id.clone()), e))? { WorkflowHandle::True } else { WorkflowHandle::False }
                }
                WorkflowNodeData::Wait(data) => {
                    tokio::time::sleep(Duration::from_millis(data.duration_ms)).await;
                    WorkflowHandle::WaitDone
                }
            };
            emit(&events, WorkflowEvent::NodeFinished { execution_id: execution_id.clone(), node_id: node.id.clone(), output });
            tokio::time::sleep(Duration::from_millis(50)).await;
            let Some(next) = next_node(&file.graph, &node.id, output) else {
                if output == WorkflowHandle::OnError { return Err((Some(node.id.clone()), "macro échouée sans branche on_error".into())); }
                return Ok(executed);
            };
            current = next;
        }
    }.await;

    match result {
        Ok(count) => {
            emit(&events, WorkflowEvent::Finished { execution_id, name: file.name, nodes_executed: count });
            Ok(count)
        }
        Err((node_id, message)) => {
            emit(&events, WorkflowEvent::Failed { execution_id, node_id, message: message.clone() });
            Err(message)
        }
    }
}

fn find_target(data: &StartNodeData) -> Result<Option<AttachedWindow>, String> {
    let mut context = data.app_context.as_deref().unwrap_or("").trim().to_lowercase();
    if context == "unknown" { context.clear(); }
    let title = data.app_title.as_deref().unwrap_or("").trim().to_lowercase();
    let title_suffix = title.rsplit_once(" — ").or_else(|| title.rsplit_once(" - ")).map(|(_, suffix)| suffix.trim()).unwrap_or(&title);
    if context.is_empty() && title.is_empty() { return Ok(None); }
    let windows = capture::list_windows(&[], true).map_err(|e| format!("liste des applications: {e}"))?;
    let found = windows.into_iter().find(|w| {
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
    let has_selector = data.app_context.as_deref().is_some_and(|v| !v.trim().is_empty() && !v.eq_ignore_ascii_case("unknown"))
        || data.app_title.as_deref().is_some_and(|v| !v.trim().is_empty());
    let mut target = if has_selector { find_target(data)? } else { fallback };
    if target.is_none() && has_selector {
        launch_start_app(data)?;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Some(found) = find_target(data)? { target = Some(found); break; }
        }
    }
    let target = target.ok_or_else(|| "application cible introuvable après lancement".to_string())?;
    if data.focus { input::focus_window(target.id); }
    Ok(target)
}
