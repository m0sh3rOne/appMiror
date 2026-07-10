//! Abstraction de capture indépendante de la plateforme.
//!
//! Backends : Windows (WGC) et Linux (X11/XWayland via x11rb). Un backend
//! PipeWire (Wayland natif) viendrait s'ajouter derrière la même API
//! `list_windows` / `start`.

use crate::protocol::WindowInfo;
use tokio::sync::watch;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::CaptureSession;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use self::linux::CaptureSession;

/// Stub pour les plateformes sans backend (le serveur compile mais renvoie
/// une erreur à l'usage).
#[cfg(not(any(windows, target_os = "linux")))]
pub struct CaptureSession;
#[cfg(not(any(windows, target_os = "linux")))]
impl CaptureSession {
    pub fn stop(self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Liste les fenêtres capturables (titre non vide).
pub fn list_windows(locked_apps: &[String], allow_virtual_desktop: bool) -> anyhow::Result<Vec<WindowInfo>> {
    #[cfg(windows)]
    {
        self::windows::list_windows(locked_apps, allow_virtual_desktop)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = allow_virtual_desktop;
        self::linux::list_windows(locked_apps)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (locked_apps, allow_virtual_desktop);
        anyhow::bail!("capture non implémentée sur cette plateforme")
    }
}

/// Démarre la capture de la fenêtre `hwnd` (HWND Windows / Window X11).
///
/// Retourne une session (qu'il faut garder en vie et `stop()` en fin) et un
/// `watch::Receiver` qui reçoit la dernière frame JPEG encodée. On utilise
/// `watch` volontairement : seule la frame la plus récente compte, les frames
/// en retard sont écrasées => latence minimale.
pub fn start(
    hwnd: isize,
    quality: u8,
    scale: f32,
    show_cursor: bool,
) -> anyhow::Result<(CaptureSession, watch::Receiver<Vec<u8>>)> {
    #[cfg(windows)]
    {
        self::windows::start(hwnd, quality, scale, show_cursor)
    }
    #[cfg(target_os = "linux")]
    {
        self::linux::start(hwnd, quality, scale, show_cursor)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (hwnd, quality, scale, show_cursor);
        anyhow::bail!("capture non implémentée sur cette plateforme")
    }
}

pub fn is_window_locked(id: isize, locked_apps: &[String]) -> bool {
    #[cfg(windows)]
    {
        self::windows::is_window_locked(id, locked_apps)
    }
    #[cfg(target_os = "linux")]
    {
        self::linux::is_window_locked(id, locked_apps)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (id, locked_apps);
        false
    }
}
