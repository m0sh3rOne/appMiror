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

use std::sync::atomic::{AtomicIsize, Ordering};

use crate::protocol::MouseButton;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEINPUT, MOUSE_EVENT_FLAGS, MapVirtualKeyW, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::System::StationsAndDesktops::{GetThreadDesktop, HDESK, SetThreadDesktop};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, ChildWindowFromPointEx, GetClientRect, GetForegroundWindow, GetGUIThreadInfo,
    GetSystemMetrics, GetWindowThreadProcessId, IsChild, IsIconic, PostMessageW, SetForegroundWindow,
    ShowWindow, CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, GUITHREADINFO,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE,
    WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

fn as_hwnd(id: isize) -> HWND {
    HWND(id as *mut core::ffi::c_void)
}

struct DesktopGuard {
    old_desk: Option<HDESK>,
}

impl DesktopGuard {
    fn new(target_hwnd: HWND) -> Self {
        unsafe {
            let tid = GetWindowThreadProcessId(target_hwnd, None);
            if tid != 0 {
                let window_desk = GetThreadDesktop(tid).ok();
                let cur_tid = GetCurrentThreadId();
                let cur_desk = GetThreadDesktop(cur_tid).ok();
                if let (Some(w_desk), Some(c_desk)) = (window_desk, cur_desk) {
                    if !w_desk.is_invalid() && w_desk != c_desk {
                        if SetThreadDesktop(w_desk).is_ok() {
                            return Self { old_desk: Some(c_desk) };
                        }
                    }
                }
            }
        }
        Self { old_desk: None }
    }
}

impl Drop for DesktopGuard {
    fn drop(&mut self) {
        if let Some(old) = self.old_desk {
            unsafe {
                let _ = SetThreadDesktop(old);
            }
        }
    }
}

pub fn focus_window(hwnd: isize) {
    let target = as_hwnd(hwnd);
    let _guard = DesktopGuard::new(target);
    unsafe {
        // 1) Restaure la fenêtre si elle est minimisée : WGC ne capture pas une
        //    fenêtre iconifiée (frames vides / taille client aberrante « 148×22 »).
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }
        // 2) `SetForegroundWindow` échoue souvent depuis un thread en arrière-plan
        //    (foreground-lock de Windows). On s'attache temporairement à la file
        //    d'entrée du thread au premier plan pour lever la restriction — c'est
        //    la parade standard.
        let fg_tid = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let our_tid = GetCurrentThreadId();
        let attached =
            fg_tid != 0 && fg_tid != our_tid && AttachThreadInput(our_tid, fg_tid, true).as_bool();
        let _ = SetForegroundWindow(target);
        let _ = BringWindowToTop(target);
        if attached {
            let _ = AttachThreadInput(our_tid, fg_tid, false);
        }
    }
}

pub fn is_foreground_window(hwnd: isize) -> bool {
    unsafe { GetForegroundWindow() == as_hwnd(hwnd) }
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
    let target = as_hwnd(hwnd);
    let _guard = DesktopGuard::new(target);
    let (ax, ay) = client_to_abs(target, x, y);
    send_mouse(
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        ax,
        ay,
        0,
    );
}

pub fn mouse_button(hwnd: isize, button: MouseButton, pressed: bool, x: i32, y: i32) {
    let target = as_hwnd(hwnd);
    let _guard = DesktopGuard::new(target);
    let (ax, ay) = client_to_abs(target, x, y);
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
    let target = as_hwnd(hwnd);
    let _guard = DesktopGuard::new(target);
    let (ax, ay) = client_to_abs(target, x, y);
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
        "NumpadMultiply" | "Asterisk" => VK_MULTIPLY.0,
        "NumpadAdd" | "Plus" => VK_ADD.0,
        "NumpadSubtract" | "Minus" => VK_SUBTRACT.0,
        "NumpadDivide" | "Slash" => VK_DIVIDE.0,
        "Equal" => VK_OEM_PLUS.0,
        _ => return None,
    };
    Some(vk)
}

pub fn key(hwnd: Option<isize>, code: &str, pressed: bool) {
    let _guard = hwnd.map(|h| DesktopGuard::new(as_hwnd(h)));
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

/// Injection Unicode indépendante du layout clavier configuré sur l'hôte.
/// Un couple key-down/key-up est émis pour chaque unité UTF-16.
pub fn text(hwnd: Option<isize>, text: &str) {
    let _guard = hwnd.map(|h| DesktopGuard::new(as_hwnd(h)));
    for unit in text.encode_utf16() {
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            let input = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: unit,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32); }
        }
    }
}

// ============================================================================
// Injection en arrière-plan via PostMessage (sans focus, sans curseur physique)
//
// Chaque message est posté dans la file de la fenêtre cible. Pour la souris, on
// résout le contrôle enfant le plus profond sous le point (les clics atteignent
// ainsi boutons/champs même en arrière-plan). Pour le clavier, on vise le
// contrôle focalisé du thread de la fenêtre et on émet WM_CHAR pour que le texte
// apparaisse réellement dans les contrôles d'édition.
//
// Limites : ne fonctionne pas pour les apps DirectX / plein écran exclusif ; la
// saisie clavier en arrière-plan dépend de l'app (certaines ignorent les
// messages synthétiques qui ne passent pas par TranslateMessage).
// ============================================================================

const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;
const MK_MBUTTON: usize = 0x0010;

/// Dernier contrôle enfant cliqué en arrière-plan (HWND en `isize`, 0 = aucun).
/// Une fenêtre en arrière-plan n'a pas de « contrôle focalisé » au sens de l'OS :
/// on route donc le clavier vers le champ que l'utilisateur distant a cliqué en
/// dernier — exactement comme on placerait le curseur avant de saisir.
static LAST_KBD_TARGET: AtomicIsize = AtomicIsize::new(0);

/// Empaquette (x,y) client dans un LPARAM (low=x, high=y), façon MAKELPARAM.
fn make_lparam(x: i32, y: i32) -> LPARAM {
    LPARAM((((y & 0xFFFF) << 16) | (x & 0xFFFF)) as isize)
}

fn post(hwnd: HWND, msg: u32, wparam: usize, lparam: LPARAM) {
    unsafe {
        let _ = PostMessageW(hwnd, msg, WPARAM(wparam), lparam);
    }
}

/// Contrôle enfant le plus profond de `root` sous le point client `pt` (relatif
/// à `root`), avec le point reprojeté dans le repère client de ce contrôle.
fn deepest_child(root: HWND, pt: POINT) -> (HWND, POINT) {
    let mut parent = root;
    let mut local = pt;
    for _ in 0..16 {
        let child = unsafe {
            ChildWindowFromPointEx(
                parent,
                local,
                CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT | CWP_SKIPDISABLED,
            )
        };
        if child.0.is_null() || child == parent {
            break;
        }
        // Reprojette le point : repère client parent -> écran -> client enfant.
        let mut sp = local;
        unsafe {
            let _ = ClientToScreen(parent, &mut sp);
            let _ = ScreenToClient(child, &mut sp);
        }
        parent = child;
        local = sp;
    }
    (parent, local)
}

pub fn mouse_move_bg(hwnd: isize, x: i32, y: i32) {
    let (target, pt) = deepest_child(as_hwnd(hwnd), POINT { x, y });
    post(target, WM_MOUSEMOVE, 0, make_lparam(pt.x, pt.y));
}

pub fn mouse_button_bg(hwnd: isize, button: MouseButton, pressed: bool, x: i32, y: i32) {
    let (target, pt) = deepest_child(as_hwnd(hwnd), POINT { x, y });
    // Un appui mémorise le champ visé : c'est la cible du clavier en arrière-plan.
    if pressed {
        LAST_KBD_TARGET.store(target.0 as isize, Ordering::Relaxed);
    }
    let lp = make_lparam(pt.x, pt.y);
    let (msg, mk) = match (button, pressed) {
        (MouseButton::Left, true) => (WM_LBUTTONDOWN, MK_LBUTTON),
        (MouseButton::Left, false) => (WM_LBUTTONUP, 0),
        (MouseButton::Right, true) => (WM_RBUTTONDOWN, MK_RBUTTON),
        (MouseButton::Right, false) => (WM_RBUTTONUP, 0),
        (MouseButton::Middle, true) => (WM_MBUTTONDOWN, MK_MBUTTON),
        (MouseButton::Middle, false) => (WM_MBUTTONUP, 0),
    };
    // Un survol préalable positionne l'état de hover du contrôle.
    post(target, WM_MOUSEMOVE, mk, lp);
    post(target, msg, mk, lp);
}

pub fn wheel_bg(hwnd: isize, delta: i32, x: i32, y: i32) {
    let root = as_hwnd(hwnd);
    let (target, _local) = deepest_child(root, POINT { x, y });
    // WM_MOUSEWHEEL attend le point en coordonnées ÉCRAN dans lParam.
    let mut sp = POINT { x, y };
    unsafe {
        let _ = ClientToScreen(root, &mut sp);
    }
    let notches = (delta * 120) as i16;
    let wparam = ((notches as u16 as usize) << 16) & 0xFFFF_0000;
    post(target, WM_MOUSEWHEEL, wparam, make_lparam(sp.x, sp.y));
}

/// Contrôle focalisé du thread propriétaire de `root` (sinon `root` lui-même).
///
/// Pour une fenêtre en arrière-plan, `GetGUIThreadInfo` ne renvoie pas le focus
/// (notion propre au thread au premier plan). On attache donc temporairement
/// notre file d'entrée à celle du thread cible (`AttachThreadInput`) : `GetFocus`
/// renvoie alors le contrôle réellement focalisé dans la cible (ex. le champ
/// d'édition), ce qui permet à `WM_CHAR` d'y insérer le texte. L'attache ne
/// modifie ni le focus ni l'ordre d'empilement — le focus de l'hôte est préservé.
fn focused_control(root: HWND) -> HWND {
    unsafe {
        let target_tid = GetWindowThreadProcessId(root, None);
        if target_tid == 0 {
            return root;
        }
        let cur_tid = GetCurrentThreadId();
        let mut focused = HWND::default();

        let attached = target_tid != cur_tid
            && AttachThreadInput(cur_tid, target_tid, true).as_bool();
        let f = GetFocus();
        if !f.0.is_null() {
            focused = f;
        }
        if attached {
            let _ = AttachThreadInput(cur_tid, target_tid, false);
        }

        // Repli : GUITHREADINFO (utile si le thread cible est au premier plan).
        if focused.0.is_null() {
            let mut gti = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            if GetGUIThreadInfo(target_tid, &mut gti).is_ok() && !gti.hwndFocus.0.is_null() {
                focused = gti.hwndFocus;
            }
        }

        if focused.0.is_null() {
            root
        } else {
            focused
        }
    }
}

/// Cible clavier en arrière-plan : le dernier champ cliqué s'il appartient
/// toujours à `root`, sinon le contrôle focalisé (cas où la fenêtre est au
/// premier plan), sinon `root`.
fn kbd_target(root: HWND) -> HWND {
    let last = LAST_KBD_TARGET.load(Ordering::Relaxed);
    if last != 0 {
        let h = HWND(last as *mut core::ffi::c_void);
        if h != root && unsafe { IsChild(root, h).as_bool() } {
            return h;
        }
    }
    focused_control(root)
}

pub fn key_bg(hwnd: isize, code: &str, pressed: bool) {
    let Some(vk) = code_to_vk(code) else {
        tracing::debug!("touche non mappée (bg): {code}");
        return;
    };
    let target = kbd_target(as_hwnd(hwnd));
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) };
    let mut lparam = 1u32 | (scan << 16); // compteur de répétition = 1 + scancode
    let msg = if pressed {
        WM_KEYDOWN
    } else {
        lparam |= (1 << 30) | (1 << 31); // bits « précédemment enfoncée » + relâche
        WM_KEYUP
    };
    // On ne poste QUE WM_KEYDOWN/WM_KEYUP : la boucle de messages de l'app cible
    // fait le TranslateMessage et produit le WM_CHAR correct pour la disposition
    // clavier active. Synthétiser nous-mêmes un WM_CHAR doublerait la saisie et
    // ignorerait la disposition.
    //
    // Limite : PostMessage ne met pas à jour l'état clavier du thread cible, donc
    // son TranslateMessage ne « voit » pas Shift enfoncé. Les caractères NON
    // shiftés (minuscules, chiffres/symboles de base de la disposition) et les
    // touches de navigation passent ; les majuscules/caractères shiftés sont
    // best-effort et généralement non rendus en arrière-plan.
    post(target, msg, vk as usize, LPARAM(lparam as isize));
}

pub fn text_bg(hwnd: isize, text: &str) {
    let target = kbd_target(as_hwnd(hwnd));
    for unit in text.encode_utf16() {
        post(target, WM_CHAR, unit as usize, LPARAM(1));
    }
}

struct ChildWindowInfo {
    class_name: String,
    text: String,
    rect: RECT,
}

unsafe extern "system" fn enum_child_callback(hwnd: HWND, lparam: LPARAM) -> windows::Win32::Foundation::BOOL {
    use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, GetClassNameW, GetWindowTextW, GetWindowRect};
    use windows::Win32::Foundation::BOOL;

    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }

    let list = &mut *(lparam.0 as *mut Vec<ChildWindowInfo>);
    
    let mut class_buf = [0u16; 256];
    let len = GetClassNameW(hwnd, &mut class_buf);
    let class_name = if len > 0 {
        String::from_utf16_lossy(&class_buf[..len as usize])
    } else {
        String::new()
    };

    let mut text_buf = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut text_buf);
    let text = if len > 0 {
        String::from_utf16_lossy(&text_buf[..len as usize])
    } else {
        String::new()
    };

    let mut rect = RECT::default();
    let _ = GetWindowRect(hwnd, &mut rect);

    list.push(ChildWindowInfo {
        class_name,
        text,
        rect,
    });

    BOOL(1)
}

pub fn inspect_ui_elements(hwnd: isize) -> Result<serde_json::Value, String> {
    use windows::Win32::UI::WindowsAndMessaging::EnumChildWindows;
    use serde_json::json;

    let parent_hwnd = as_hwnd(hwnd);
    let (parent_w, parent_h) = client_size(hwnd).ok_or_else(|| "fenêtre introuvable".to_string())?;

    let mut children = Vec::<ChildWindowInfo>::new();
    unsafe {
        let lparam = LPARAM(&mut children as *mut _ as isize);
        let _ = EnumChildWindows(parent_hwnd, Some(enum_child_callback), lparam);
    }

    let mut elements = Vec::new();
    for child in children {
        let mut pt_top_left = POINT { x: child.rect.left, y: child.rect.top };
        let mut pt_bottom_right = POINT { x: child.rect.right, y: child.rect.bottom };
        unsafe {
            let _ = ScreenToClient(parent_hwnd, &mut pt_top_left);
            let _ = ScreenToClient(parent_hwnd, &mut pt_bottom_right);
        }

        let left = pt_top_left.x;
        let top = pt_top_left.y;
        let right = pt_bottom_right.x;
        let bottom = pt_bottom_right.y;

        let w = right - left;
        let h = bottom - top;

        if w <= 0 || h <= 0 {
            continue;
        }

        let cx = left + w / 2;
        let cy = top + h / 2;

        let x_norm = cx as f64 / parent_w.max(1) as f64;
        let y_norm = cy as f64 / parent_h.max(1) as f64;
        let w_norm = w as f64 / parent_w.max(1) as f64;
        let h_norm = h as f64 / parent_h.max(1) as f64;

        let x_norm = (x_norm * 1000.0).round() / 1000.0;
        let y_norm = (y_norm * 1000.0).round() / 1000.0;
        let w_norm = (w_norm * 1000.0).round() / 1000.0;
        let h_norm = (h_norm * 1000.0).round() / 1000.0;

        elements.push(json!({
            "class": child.class_name,
            "text": child.text,
            "x": x_norm,
            "y": y_norm,
            "width": w_norm,
            "height": h_norm
        }));
    }

    Ok(json!(elements))
}
