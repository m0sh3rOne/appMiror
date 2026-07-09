//! Capture Linux via X11 (`x11rb`) : `GetImage` périodique sur la fenêtre.
//!
//! Fonctionne sous X11 natif et sous XWayland (fenêtres X). Les fenêtres
//! Wayland natives nécessiteraient un backend PipeWire/xdg-desktop-portal —
//! prochaine étape, derrière la même façade `capture::start`.
//!
//! Même contrat que le backend Windows : un thread de capture encode chaque
//! frame en JPEG et la pousse dans un canal `watch` (latest-frame-wins).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
use tokio::sync::watch;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

use crate::protocol::WindowInfo;

/// Cadence de capture (X11 n'a pas de notification de frame comme WGC).
const FRAME_INTERVAL: Duration = Duration::from_millis(33); // ~30 fps

/// Contrôleur externe : garder vivant, appeler `stop()` pour arrêter.
pub struct CaptureSession {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CaptureSession {
    pub fn stop(mut self) -> anyhow::Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        Ok(())
    }
}

fn atom(conn: &RustConnection, name: &str) -> anyhow::Result<u32> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

/// Titre d'une fenêtre : `_NET_WM_NAME` (UTF-8), sinon `WM_NAME` (legacy).
fn window_title(conn: &RustConnection, win: Window, net_wm_name: u32, utf8: u32) -> String {
    let modern = conn
        .get_property(false, win, net_wm_name, utf8, 0, u32::MAX)
        .ok()
        .and_then(|c| c.reply().ok())
        .filter(|r| !r.value.is_empty())
        .map(|r| String::from_utf8_lossy(&r.value).into_owned());
    if let Some(t) = modern {
        return t;
    }
    conn.get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, u32::MAX)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| String::from_utf8_lossy(&r.value).into_owned())
        .unwrap_or_default()
}

fn get_process_name(conn: &RustConnection, win: Window, net_wm_pid: u32) -> Option<String> {
    let reply = conn.get_property(false, win, net_wm_pid, AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok()?;
    let pid = reply.value32()?.next()?;
    std::fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|s| s.trim().to_string())
        .ok()
}

/// Fenêtres gérées par le WM (`_NET_CLIENT_LIST` sur la racine).
pub fn list_windows(locked_apps: &[String]) -> anyhow::Result<Vec<WindowInfo>> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;

    let client_list = atom(&conn, "_NET_CLIENT_LIST")?;
    let net_wm_name = atom(&conn, "_NET_WM_NAME")?;
    let utf8 = atom(&conn, "UTF8_STRING")?;
    let net_wm_pid = atom(&conn, "_NET_WM_PID")?;

    let reply = conn
        .get_property(false, root, client_list, AtomEnum::WINDOW, 0, u32::MAX)?
        .reply()?;

    let mut out = Vec::new();
    for win in reply.value32().into_iter().flatten() {
        let title = window_title(&conn, win, net_wm_name, utf8);
        if title.trim().is_empty() {
            continue;
        }
        let proc_name = get_process_name(&conn, win, net_wm_pid);
        
        let mut locked = false;
        let title_lower = title.to_lowercase();
        let proc_lower = proc_name.as_ref().map(|p| p.to_lowercase());
        
        for pattern in locked_apps {
            let pattern_lower = pattern.to_lowercase();
            if title_lower.contains(&pattern_lower) {
                locked = true;
                break;
            }
            if let Some(ref p) = proc_lower {
                if p.contains(&pattern_lower) {
                    locked = true;
                    break;
                }
            }
        }

        out.push(WindowInfo {
            id: win as isize,
            title,
            locked,
        });
    }
    Ok(out)
}

fn downscale_rgb(rgb: &[u8], w: u32, h: u32, scale: f32) -> (Vec<u8>, u32, u32) {
    if scale >= 0.99 {
        return (rgb.to_vec(), w, h);
    }
    let new_w = ((w as f32 * scale) as u32).max(1);
    let new_h = ((h as f32 * scale) as u32).max(1);
    let mut out = Vec::with_capacity((new_w * new_h * 3) as usize);
    for y in 0..new_h {
        let src_y = (((y as f32 / scale) as u32)).min(h - 1);
        let src_row_offset = (src_y * w * 3) as usize;
        for x in 0..new_w {
            let src_x = (((x as f32 / scale) as u32)).min(w - 1);
            let src_pixel_offset = src_row_offset + (src_x * 3) as usize;
            out.push(rgb[src_pixel_offset]);
            out.push(rgb[src_pixel_offset + 1]);
            out.push(rgb[src_pixel_offset + 2]);
        }
    }
    (out, new_w, new_h)
}

pub fn start(
    hwnd: isize,
    quality: u8,
    scale: f32,
    // `GetImage` X11 ne compose pas le curseur dans l'image de la fenêtre :
    // l'option n'a donc pas d'effet ici (contrairement à WGC sous Windows).
    _show_cursor: bool,
) -> anyhow::Result<(CaptureSession, watch::Receiver<Vec<u8>>)> {
    let win = hwnd as Window;
    let (tx, rx) = watch::channel(Vec::new());

    // Connexion ouverte ici pour échouer tout de suite si X est indisponible.
    let (conn, _) = x11rb::connect(None)?;
    // Vérifie que la fenêtre existe encore avant de lancer le thread.
    conn.get_geometry(win)?
        .reply()
        .map_err(|e| anyhow::anyhow!("fenêtre introuvable: {e}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    let thread = std::thread::spawn(move || {
        capture_loop(conn, win, quality, scale, tx, stop_thread);
    });

    Ok((
        CaptureSession {
            stop,
            thread: Some(thread),
        },
        rx,
    ))
}

fn capture_loop(
    conn: RustConnection,
    win: Window,
    quality: u8,
    scale: f32,
    tx: watch::Sender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        match grab_frame(&conn, win, quality, scale) {
            Ok(jpeg) => {
                // Plus de receiver => la session est terminée côté serveur.
                if tx.send(jpeg).is_err() {
                    break;
                }
            }
            Err(e) => {
                tracing::warn!("capture X11 interrompue: {e}");
                break;
            }
        }
        std::thread::sleep(FRAME_INTERVAL);
    }
    tracing::info!("capture X11 terminée");
}

fn grab_frame(conn: &RustConnection, win: Window, quality: u8, scale: f32) -> anyhow::Result<Vec<u8>> {
    let geo = conn.get_geometry(win)?.reply()?;
    let img = conn
        .get_image(ImageFormat::Z_PIXMAP, win, 0, 0, geo.width, geo.height, !0)?
        .reply()?;

    let (w, h) = (geo.width as u32, geo.height as u32);
    // ZPixmap profondeur 24/32 : 4 octets/pixel, ordre B,G,R,X en little-endian.
    anyhow::ensure!(
        img.data.len() >= (w * h * 4) as usize,
        "format de pixmap inattendu (profondeur {})",
        img.depth
    );

    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for px in img.data.chunks_exact(4).take((w * h) as usize) {
        rgb.push(px[2]);
        rgb.push(px[1]);
        rgb.push(px[0]);
    }

    let (scaled_rgb, final_w, final_h) = if scale < 0.99 {
        downscale_rgb(&rgb, w, h, scale)
    } else {
        (rgb, w, h)
    };

    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, quality)
        .encode(&scaled_rgb, final_w, final_h, ExtendedColorType::Rgb8)?;
    Ok(jpeg)
}

pub fn is_window_locked(win_id: isize, locked_apps: &[String]) -> bool {
    if locked_apps.is_empty() {
        return false;
    }
    let (conn, _) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let win = win_id as Window;
    let net_wm_name = match atom(&conn, "_NET_WM_NAME") {
        Ok(a) => a,
        Err(_) => return false,
    };
    let utf8 = match atom(&conn, "UTF8_STRING") {
        Ok(a) => a,
        Err(_) => return false,
    };
    let net_wm_pid = match atom(&conn, "_NET_WM_PID") {
        Ok(a) => a,
        Err(_) => return false,
    };
    let title = window_title(&conn, win, net_wm_name, utf8);
    let proc_name = get_process_name(&conn, win, net_wm_pid);
    
    let title_lower = title.to_lowercase();
    let proc_lower = proc_name.as_ref().map(|p| p.to_lowercase());
    
    for pattern in locked_apps {
        let pattern_lower = pattern.to_lowercase();
        if title_lower.contains(&pattern_lower) {
            return true;
        }
        if let Some(ref p) = proc_lower {
            if p.contains(&pattern_lower) {
                return true;
            }
        }
    }
    false
}
