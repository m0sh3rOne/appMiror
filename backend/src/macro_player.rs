//! Lecture séquentielle non bloquante des workflows via l'injection background.

use std::time::Duration;

use crate::app::AttachedWindow;
use crate::protocol::{MacroFile, MacroStep};
use crate::input;

fn denorm(target: &AttachedWindow, x: f64, y: f64) -> (i32, i32) {
    let px = (x.clamp(0.0, 1.0) * (target.width.max(1) - 1) as f64).round() as i32;
    let py = (y.clamp(0.0, 1.0) * (target.height.max(1) - 1) as f64).round() as i32;
    (px, py)
}

pub async fn play(file: &MacroFile, mut target: AttachedWindow) -> Result<usize, String> {
    let (width, height) = input::client_size(target.id).ok_or_else(|| format!("fenêtre cible {} introuvable", target.id))?;
    target.width = width;
    target.height = height;
    for step in &file.steps {
        match step {
            MacroStep::MouseMove { x, y } => { let (x, y) = denorm(&target, *x, *y); input::mouse_move_bg(target.id, x, y); }
            MacroStep::MouseButton { button, pressed, x, y } => { let (x, y) = denorm(&target, *x, *y); input::mouse_button_bg(target.id, *button, *pressed, x, y); }
            MacroStep::Wheel { delta, x, y } => { let (x, y) = denorm(&target, *x, *y); input::wheel_bg(target.id, *delta, x, y); }
            MacroStep::Key { code, pressed } => input::key_bg(target.id, code, *pressed),
            MacroStep::Text { text } => input::text_bg(target.id, text),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(file.steps.len())
}
