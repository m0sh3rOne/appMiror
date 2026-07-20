//! Lecture séquentielle non bloquante des workflows via l'injection background.

use std::time::Duration;

use crate::app::AttachedWindow;
use crate::protocol::{InjectionMode, MacroFile, MacroStep};
use crate::input;

fn denorm(target: &AttachedWindow, x: f64, y: f64) -> (i32, i32) {
    let px = (x.clamp(0.0, 1.0) * (target.width.max(1) - 1) as f64).round() as i32;
    let py = (y.clamp(0.0, 1.0) * (target.height.max(1) - 1) as f64).round() as i32;
    (px, py)
}

pub async fn play(file: &MacroFile, target: AttachedWindow) -> Result<usize, String> {
    play_with_mode(file, target, InjectionMode::Background).await
}

pub async fn play_with_mode(file: &MacroFile, mut target: AttachedWindow, mode: InjectionMode) -> Result<usize, String> {
    let (width, height) = input::client_size(target.id).ok_or_else(|| format!("fenêtre cible {} introuvable", target.id))?;
    target.width = width;
    target.height = height;
    for step in &file.steps {
        match step {
            MacroStep::MouseMove { x, y } => { let (x, y) = denorm(&target, *x, *y); if mode == InjectionMode::Background { input::mouse_move_bg(target.id, x, y) } else { input::mouse_move(target.id, x, y) } }
            MacroStep::MouseButton { button, pressed, x, y } => { let (x, y) = denorm(&target, *x, *y); if mode == InjectionMode::Background { input::mouse_button_bg(target.id, *button, *pressed, x, y) } else { input::mouse_button(target.id, *button, *pressed, x, y) } }
            MacroStep::Wheel { delta, x, y } => { let (x, y) = denorm(&target, *x, *y); if mode == InjectionMode::Background { input::wheel_bg(target.id, *delta, x, y) } else { input::wheel(target.id, *delta, x, y) } }
            MacroStep::Key { code, pressed } => if mode == InjectionMode::Background { input::key_bg(target.id, code, *pressed) } else { input::key(Some(target.id), code, *pressed) },
            MacroStep::Text { text } => if mode == InjectionMode::Background { input::text_bg(target.id, text) } else { let _ = crate::mcp::type_text(Some(target.id), text); },
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(file.steps.len())
}
