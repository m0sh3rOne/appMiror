//! Injection via `SendInput` (Win32).
//!
//! Stratégie POC : on convertit les coordonnées **client de la fenêtre** en
//! coordonnées **absolues du bureau virtuel** (0..65535) et on déplace le vrai
//! curseur. `SendInput` envoie à la fenêtre au premier plan, donc on appelle
//! `SetForegroundWindow` à la sélection.
//!
//! Alternative (non retenue ici) : `PostMessage(hwnd, WM_MOUSEMOVE, ...)` pour
//! injecter en arrière-plan sans voler le focus — ne marche pas avec les apps
//! DirectX / plein écran exclusif.

use crate::protocol::MouseButton;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEINPUT, MOUSE_EVENT_FLAGS, MapVirtualKeyW, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetSystemMetrics, SetForegroundWindow, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

fn as_hwnd(id: isize) -> HWND {
    HWND(id as *mut core::ffi::c_void)
}

pub fn focus_window(hwnd: isize) {
    unsafe {
        let _ = SetForegroundWindow(as_hwnd(hwnd));
    }
}

pub fn client_size(hwnd: isize) -> Option<(i32, i32)> {
    let mut rect = RECT::default();
    unsafe {
        GetClientRect(as_hwnd(hwnd), &mut rect).ok()?;
    }
    Some((rect.right - rect.left, rect.bottom - rect.top))
}

/// Convertit (x,y) client -> coords absolues 0..65535 du bureau virtuel.
fn client_to_abs(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut pt = POINT { x, y };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut pt);
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
        let ax = ((pt.x - vx) as f64 * 65535.0 / (vw - 1).max(1) as f64).round() as i32;
        let ay = ((pt.y - vy) as f64 * 65535.0 / (vh - 1).max(1) as f64).round() as i32;
        (ax, ay)
    }
}

fn send_mouse(flags: MOUSE_EVENT_FLAGS, abs_x: i32, abs_y: i32, data: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: abs_x,
                dy: abs_y,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

pub fn mouse_move(hwnd: isize, x: i32, y: i32) {
    let (ax, ay) = client_to_abs(as_hwnd(hwnd), x, y);
    send_mouse(
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        ax,
        ay,
        0,
    );
}

pub fn mouse_button(hwnd: isize, button: MouseButton, pressed: bool, x: i32, y: i32) {
    let (ax, ay) = client_to_abs(as_hwnd(hwnd), x, y);
    let btn = match (button, pressed) {
        (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
        (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
        (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
    };
    // Positionne puis clique dans le même geste (move absolu + bouton).
    send_mouse(
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | btn,
        ax,
        ay,
        0,
    );
}

pub fn wheel(hwnd: isize, delta: i32, x: i32, y: i32) {
    let (ax, ay) = client_to_abs(as_hwnd(hwnd), x, y);
    // WHEEL_DELTA = 120 par cran.
    send_mouse(
        MOUSEEVENTF_WHEEL | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        ax,
        ay,
        delta * 120,
    );
}

/// Mappe un `KeyboardEvent.code` vers une VIRTUAL_KEY Win32.
/// Table minimale — étendre selon les besoins.
fn code_to_vk(code: &str) -> Option<u16> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let vk = match code {
        c if c.starts_with("Key") && c.len() == 4 => {
            // "KeyA".."KeyZ" -> 'A'..'Z'
            c.as_bytes()[3] as u16
        }
        c if c.starts_with("Digit") && c.len() == 6 => {
            // "Digit0".."Digit9" -> '0'..'9'
            c.as_bytes()[5] as u16
        }
        "Enter" => VK_RETURN.0,
        "Escape" => VK_ESCAPE.0,
        "Backspace" => VK_BACK.0,
        "Tab" => VK_TAB.0,
        "Space" => VK_SPACE.0,
        "ArrowLeft" => VK_LEFT.0,
        "ArrowRight" => VK_RIGHT.0,
        "ArrowUp" => VK_UP.0,
        "ArrowDown" => VK_DOWN.0,
        "ShiftLeft" | "ShiftRight" => VK_SHIFT.0,
        "ControlLeft" | "ControlRight" => VK_CONTROL.0,
        "AltLeft" | "AltRight" => VK_MENU.0,
        "Delete" => VK_DELETE.0,
        "Home" => VK_HOME.0,
        "End" => VK_END.0,
        _ => return None,
    };
    Some(vk)
}

pub fn key(code: &str, pressed: bool) {
    let Some(vk) = code_to_vk(code) else {
        tracing::debug!("touche non mappée: {code}");
        return;
    };
    // On envoie par scancode (plus fiable pour les jeux/apps bas-niveau).
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = KEYEVENTF_SCANCODE;
    if !pressed {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
    let _ = KEYBD_EVENT_FLAGS(0); // silence unused import warning path
}
