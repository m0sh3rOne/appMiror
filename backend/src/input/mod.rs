//! Injection d'événements souris/clavier, abstraite par plateforme.
//!
//! Backends : Windows (`SendInput`) et Linux (XTest — pas de root requis,
//! contrairement à `/dev/uinput`).

use crate::protocol::MouseButton;

#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(windows)]
use self::windows as imp;
#[cfg(target_os = "linux")]
use self::linux as imp;

/// Amène la fenêtre au premier plan (l'injection cible la fenêtre focalisée).
pub fn focus_window(hwnd: isize) {
    #[cfg(any(windows, target_os = "linux"))]
    imp::focus_window(hwnd);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = hwnd;
}

/// Dimensions client (px) de la fenêtre — repère natif renvoyé au frontend.
pub fn client_size(hwnd: isize) -> Option<(i32, i32)> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        imp::client_size(hwnd)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = hwnd;
        None
    }
}

pub fn mouse_move(hwnd: isize, x: i32, y: i32) {
    #[cfg(any(windows, target_os = "linux"))]
    imp::mouse_move(hwnd, x, y);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (hwnd, x, y);
}

pub fn mouse_button(hwnd: isize, button: MouseButton, pressed: bool, x: i32, y: i32) {
    #[cfg(any(windows, target_os = "linux"))]
    imp::mouse_button(hwnd, button, pressed, x, y);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (hwnd, button, pressed, x, y);
}

pub fn wheel(hwnd: isize, delta: i32, x: i32, y: i32) {
    #[cfg(any(windows, target_os = "linux"))]
    imp::wheel(hwnd, delta, x, y);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (hwnd, delta, x, y);
}

pub fn key(code: &str, pressed: bool) {
    #[cfg(any(windows, target_os = "linux"))]
    imp::key(code, pressed);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (code, pressed);
}

// ── Injection en arrière-plan (sans focus) ──────────────────────────────────
//
// Ces variantes ciblent directement le HWND via `PostMessage` (Windows) : le
// curseur physique de l'hôte ne bouge pas et le focus n'est pas volé. Sous
// Linux, XTest est global (pas de vrai ciblage par fenêtre) : on retombe sur
// l'injection au premier plan — le mode « arrière-plan » y est donc un no-op
// fonctionnel, documenté comme tel.

pub fn mouse_move_bg(hwnd: isize, x: i32, y: i32) {
    #[cfg(windows)]
    imp::mouse_move_bg(hwnd, x, y);
    #[cfg(target_os = "linux")]
    imp::mouse_move(hwnd, x, y);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (hwnd, x, y);
}

pub fn mouse_button_bg(hwnd: isize, button: MouseButton, pressed: bool, x: i32, y: i32) {
    #[cfg(windows)]
    imp::mouse_button_bg(hwnd, button, pressed, x, y);
    #[cfg(target_os = "linux")]
    imp::mouse_button(hwnd, button, pressed, x, y);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (hwnd, button, pressed, x, y);
}

pub fn wheel_bg(hwnd: isize, delta: i32, x: i32, y: i32) {
    #[cfg(windows)]
    imp::wheel_bg(hwnd, delta, x, y);
    #[cfg(target_os = "linux")]
    imp::wheel(hwnd, delta, x, y);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (hwnd, delta, x, y);
}

pub fn key_bg(hwnd: isize, code: &str, pressed: bool) {
    #[cfg(windows)]
    imp::key_bg(hwnd, code, pressed);
    #[cfg(target_os = "linux")]
    {
        let _ = hwnd;
        imp::key(code, pressed);
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (hwnd, code, pressed);
}

pub fn inspect_ui_elements(hwnd: isize) -> Result<serde_json::Value, String> {
    #[cfg(windows)]
    {
        imp::inspect_ui_elements(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        Ok(serde_json::json!([]))
    }
}

