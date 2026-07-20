//! Lecture séquentielle non bloquante des macros : étapes brutes (souris /
//! clavier, injection background ou foreground) et étapes sémantiques
//! (activation d'éléments de layout, attente d'état, sous-macros), avec
//! substitution des paramètres `{{nom}}`.

use std::collections::HashMap;
use std::time::Duration;

use crate::app::AttachedWindow;
use crate::input;
use crate::protocol::{InjectionMode, LayoutFile, MacroFile, MacroParamKind, MacroStep, UiStateFile};

const MAX_CALL_DEPTH: usize = 3;

fn denorm(target: &AttachedWindow, x: f64, y: f64) -> (i32, i32) {
    let px = (x.clamp(0.0, 1.0) * (target.width.max(1) - 1) as f64).round() as i32;
    let py = (y.clamp(0.0, 1.0) * (target.height.max(1) - 1) as f64).round() as i32;
    (px, py)
}

/// Valeurs effectives des paramètres : fournies, sinon défaut déclaré.
/// Erreur si un paramètre déclaré reste sans valeur ni défaut.
fn resolve_params(file: &MacroFile, provided: &HashMap<String, String>) -> Result<HashMap<String, String>, String> {
    let mut values = HashMap::new();
    for param in &file.params {
        let value = provided
            .get(&param.name)
            .cloned()
            .or_else(|| param.default.clone())
            .ok_or_else(|| format!("paramètre '{}' sans valeur ni défaut", param.name))?;
        if param.kind == MacroParamKind::Choice && !param.choices.contains(&value) {
            return Err(format!("paramètre '{}': valeur '{value}' hors de {:?}", param.name, param.choices));
        }
        values.insert(param.name.clone(), value);
    }
    Ok(values)
}

/// Remplace chaque `{{nom}}` par la valeur du paramètre correspondant.
fn subst(input: &str, values: &HashMap<String, String>) -> Result<String, String> {
    if !input.contains("{{") {
        return Ok(input.to_string());
    }
    let mut out = input.to_string();
    for (name, value) in values {
        out = out.replace(&format!("{{{{{name}}}}}"), value);
    }
    if out.contains("{{") {
        return Err(format!("placeholder non résolu dans: {input}"));
    }
    Ok(out)
}

/// Un paramètre secret ne doit jamais apparaître dans logs/erreurs.
fn is_secret_value(file: &MacroFile, values: &HashMap<String, String>, text: &str) -> bool {
    file.params.iter().any(|p| {
        p.kind == MacroParamKind::Secret
            && values.get(&p.name).is_some_and(|v| !v.is_empty() && text.contains(v.as_str()))
    })
}

/// Layouts + états du contexte, chargés paresseusement à la première étape
/// sémantique (les macros purement brutes n'en ont pas besoin).
struct SemanticContext {
    layouts: Vec<LayoutFile>,
    states: Vec<UiStateFile>,
}

async fn semantic_context(context: &str) -> SemanticContext {
    SemanticContext {
        layouts: crate::layout_store::load_context(context).await.unwrap_or_default(),
        states: crate::state_store::load_context(context).await.unwrap_or_default(),
    }
}

pub async fn play(file: &MacroFile, target: AttachedWindow) -> Result<usize, String> {
    play_with_mode(file, target, InjectionMode::Background).await
}

pub async fn play_with_mode(file: &MacroFile, target: AttachedWindow, mode: InjectionMode) -> Result<usize, String> {
    play_full(file, target, mode, &HashMap::new(), 0).await
}

pub async fn play_full(
    file: &MacroFile,
    mut target: AttachedWindow,
    mode: InjectionMode,
    provided: &HashMap<String, String>,
    depth: usize,
) -> Result<usize, String> {
    if depth > MAX_CALL_DEPTH {
        return Err(format!("profondeur d'appel de macros > {MAX_CALL_DEPTH} (cycle ?)"));
    }
    let (width, height) = input::client_size(target.id).ok_or_else(|| format!("fenêtre cible {} introuvable", target.id))?;
    target.width = width;
    target.height = height;
    let values = resolve_params(file, provided)?;
    let mut semantic: Option<SemanticContext> = None;

    for (index, step) in file.steps.iter().enumerate() {
        let step_err = |e: String| format!("étape {} ({}): {e}", index + 1, step_name(step));
        match step {
            MacroStep::MouseMove { x, y } => { let (x, y) = denorm(&target, *x, *y); if mode == InjectionMode::Background { input::mouse_move_bg(target.id, x, y) } else { input::mouse_move(target.id, x, y) } }
            MacroStep::MouseButton { button, pressed, x, y } => { let (x, y) = denorm(&target, *x, *y); if mode == InjectionMode::Background { input::mouse_button_bg(target.id, *button, *pressed, x, y) } else { input::mouse_button(target.id, *button, *pressed, x, y) } }
            MacroStep::Wheel { delta, x, y } => { let (x, y) = denorm(&target, *x, *y); if mode == InjectionMode::Background { input::wheel_bg(target.id, *delta, x, y) } else { input::wheel(target.id, *delta, x, y) } }
            MacroStep::Key { code, pressed } => if mode == InjectionMode::Background { input::key_bg(target.id, code, *pressed) } else { input::key(Some(target.id), code, *pressed) },
            MacroStep::Text { text } => {
                let text = subst(text, &values).map_err(step_err)?;
                if mode == InjectionMode::Background { input::text_bg(target.id, &text) } else { input::text(Some(target.id), &text); }
            }
            MacroStep::ActivateElement { element, layout, strategy, verify, verification } => {
                let element = subst(element, &values).map_err(step_err)?;
                let strategy = crate::activate::Strategy::parse(strategy.as_deref()).map_err(step_err)?;
                let verification = *verification;
                if semantic.is_none() { semantic = Some(semantic_context(&file.context).await); }
                let ctx = semantic.as_ref().unwrap();
                crate::activate::activate_element(
                    target,
                    &ctx.layouts,
                    &ctx.states,
                    layout.as_deref(),
                    &element,
                    strategy,
                    verify.unwrap_or(true),
                    None,
                    verification,
                )
                .await
                .map_err(|e| {
                    // Ne jamais refléter un secret via le rapport d'activation.
                    if is_secret_value(file, &values, &e) { step_err("échec d'activation (détails masqués: secret)".into()) } else { step_err(e) }
                })?;
            }
            MacroStep::TypeIntoElement { element, text, layout } => {
                let element = subst(element, &values).map_err(step_err)?;
                let text = subst(text, &values).map_err(step_err)?;
                if semantic.is_none() { semantic = Some(semantic_context(&file.context).await); }
                let ctx = semantic.as_ref().unwrap();
                crate::activate::type_into_element(target, &ctx.layouts, layout.as_deref(), &element, &text)
                    .await
                    .map_err(|e| {
                        if is_secret_value(file, &values, &e) || is_secret_value(file, &values, &text) {
                            step_err("échec de saisie (détails masqués: secret)".into())
                        } else {
                            step_err(e)
                        }
                    })?;
            }
            MacroStep::WaitForState { state, timeout_ms } => {
                let state = subst(state, &values).map_err(step_err)?;
                if semantic.is_none() { semantic = Some(semantic_context(&file.context).await); }
                let ctx = semantic.as_ref().unwrap();
                crate::detect::wait_for_state_on_window(target.id, &ctx.states, &state, timeout_ms.unwrap_or(5_000))
                    .await
                    .map(|_| ())
                    .map_err(step_err)?;
            }
            MacroStep::WaitMs { duration_ms } => {
                tokio::time::sleep(Duration::from_millis((*duration_ms).min(300_000))).await;
            }
            MacroStep::CallMacro { name, context } => {
                let name = subst(name, &values).map_err(step_err)?;
                let sub = match context.as_deref() {
                    Some(ctx) => crate::macro_store::load_named(ctx, &name).await,
                    None => crate::macro_store::load_named(&file.context, &name)
                        .await
                        .or(crate::macro_store::load_by_name(&name).await),
                }
                .map_err(step_err)?;
                Box::pin(play_full(&sub, target, mode, provided, depth + 1)).await.map_err(step_err)?;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(file.steps.len())
}

fn step_name(step: &MacroStep) -> &'static str {
    match step {
        MacroStep::MouseMove { .. } => "mouse_move",
        MacroStep::MouseButton { .. } => "mouse_button",
        MacroStep::Wheel { .. } => "wheel",
        MacroStep::Key { .. } => "key",
        MacroStep::Text { .. } => "text",
        MacroStep::ActivateElement { .. } => "activate_element",
        MacroStep::TypeIntoElement { .. } => "type_into_element",
        MacroStep::WaitForState { .. } => "wait_for_state",
        MacroStep::WaitMs { .. } => "wait_ms",
        MacroStep::CallMacro { .. } => "call_macro",
    }
}
