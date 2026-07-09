//! Injection Linux via l'extension XTest (`x11rb`).
//!
//! XTest injecte au niveau du serveur X : pas besoin de root ni de
//! `/dev/uinput`, et ça fonctionne aussi sous XWayland pour les applis X.
//! Comme `SendInput` sous Windows, les événements vont à la fenêtre focalisée,
//! d'où `focus_window` à la sélection.
//!
//! Le clavier passe par les keycodes X = keycode evdev + 8 (convention xkb
//! standard sous les serveurs X modernes evdev/libinput).

use std::sync::OnceLock;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConfigureWindowAux, ConnectionExt, InputFocus, StackMode, Window, BUTTON_PRESS_EVENT,
    BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as XTestConnectionExt;
use x11rb::rust_connection::RustConnection;

use crate::protocol::MouseButton;

/// Connexion X globale (RustConnection est thread-safe), ouverte à la demande.
fn conn() -> Option<&'static (RustConnection, Window)> {
    static CONN: OnceLock<Option<(RustConnection, Window)>> = OnceLock::new();
    CONN.get_or_init(|| match x11rb::connect(None) {
        Ok((c, screen_num)) => {
            let root = c.setup().roots[screen_num].root;
            Some((c, root))
        }
        Err(e) => {
            tracing::warn!("connexion X impossible (injection désactivée): {e}");
            None
        }
    })
    .as_ref()
}

pub fn focus_window(hwnd: isize) {
    let Some((conn, _)) = conn() else { return };
    let win = hwnd as Window;
    let _ = conn.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    let _ = conn.set_input_focus(InputFocus::POINTER_ROOT, win, x11rb::CURRENT_TIME);
    let _ = conn.flush();
}

pub fn client_size(hwnd: isize) -> Option<(i32, i32)> {
    let (conn, _) = conn()?;
    let geo = conn.get_geometry(hwnd as Window).ok()?.reply().ok()?;
    Some((geo.width as i32, geo.height as i32))
}

/// (x,y) client de la fenêtre -> coordonnées racine (écran).
fn client_to_root(conn: &RustConnection, root: Window, win: Window, x: i32, y: i32) -> (i16, i16) {
    conn.translate_coordinates(win, root, x as i16, y as i16)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| (r.dst_x, r.dst_y))
        .unwrap_or((x as i16, y as i16))
}

fn fake_motion(conn: &RustConnection, root: Window, rx: i16, ry: i16) {
    let _ = conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, root, rx, ry, 0);
}

pub fn mouse_move(hwnd: isize, x: i32, y: i32) {
    let Some((conn, root)) = conn() else { return };
    let (rx, ry) = client_to_root(conn, *root, hwnd as Window, x, y);
    fake_motion(conn, *root, rx, ry);
    let _ = conn.flush();
}

pub fn mouse_button(hwnd: isize, button: MouseButton, pressed: bool, x: i32, y: i32) {
    let Some((conn, root)) = conn() else { return };
    let (rx, ry) = client_to_root(conn, *root, hwnd as Window, x, y);
    let detail = match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
    };
    let kind = if pressed { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT };
    // Positionne puis clique, comme le backend Windows.
    fake_motion(conn, *root, rx, ry);
    let _ = conn.xtest_fake_input(kind, detail, 0, *root, rx, ry, 0);
    let _ = conn.flush();
}

pub fn wheel(hwnd: isize, delta: i32, x: i32, y: i32) {
    let Some((conn, root)) = conn() else { return };
    let (rx, ry) = client_to_root(conn, *root, hwnd as Window, x, y);
    // Molette X11 = boutons 4 (haut) / 5 (bas), un clic par cran.
    let detail = if delta > 0 { 4 } else { 5 };
    fake_motion(conn, *root, rx, ry);
    for _ in 0..delta.unsigned_abs().min(10) {
        let _ = conn.xtest_fake_input(BUTTON_PRESS_EVENT, detail, 0, *root, rx, ry, 0);
        let _ = conn.xtest_fake_input(BUTTON_RELEASE_EVENT, detail, 0, *root, rx, ry, 0);
    }
    let _ = conn.flush();
}

/// Mappe un `KeyboardEvent.code` vers un keycode evdev (input-event-codes.h).
/// Le keycode X correspondant est `evdev + 8`. Table minimale, comme Windows.
fn code_to_evdev(code: &str) -> Option<u8> {
    let k: u8 = match code {
        "KeyA" => 30, "KeyB" => 48, "KeyC" => 46, "KeyD" => 32, "KeyE" => 18,
        "KeyF" => 33, "KeyG" => 34, "KeyH" => 35, "KeyI" => 23, "KeyJ" => 36,
        "KeyK" => 37, "KeyL" => 38, "KeyM" => 50, "KeyN" => 49, "KeyO" => 24,
        "KeyP" => 25, "KeyQ" => 16, "KeyR" => 19, "KeyS" => 31, "KeyT" => 20,
        "KeyU" => 22, "KeyV" => 47, "KeyW" => 17, "KeyX" => 45, "KeyY" => 21,
        "KeyZ" => 44,
        "Digit1" => 2, "Digit2" => 3, "Digit3" => 4, "Digit4" => 5, "Digit5" => 6,
        "Digit6" => 7, "Digit7" => 8, "Digit8" => 9, "Digit9" => 10, "Digit0" => 11,
        "Enter" => 28,
        "Escape" => 1,
        "Backspace" => 14,
        "Tab" => 15,
        "Space" => 57,
        "ArrowLeft" => 105,
        "ArrowRight" => 106,
        "ArrowUp" => 103,
        "ArrowDown" => 108,
        "ShiftLeft" => 42,
        "ShiftRight" => 54,
        "ControlLeft" => 29,
        "ControlRight" => 97,
        "AltLeft" => 56,
        "AltRight" => 100,
        "Delete" => 111,
        "Home" => 102,
        "End" => 107,
        _ => return None,
    };
    Some(k)
}

pub fn key(code: &str, pressed: bool) {
    let Some((conn, root)) = conn() else { return };
    let Some(evdev) = code_to_evdev(code) else {
        tracing::debug!("touche non mappée: {code}");
        return;
    };
    let keycode = evdev + 8;
    let kind = if pressed { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT };
    let _ = conn.xtest_fake_input(kind, keycode, 0, *root, 0, 0, 0);
    let _ = conn.flush();
}
