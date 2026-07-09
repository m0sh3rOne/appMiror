//! Capture via Windows Graphics Capture (crate `windows-capture`).
//!
//! Le pipeline WGC tourne sur son propre thread (`start_free_threaded`). À
//! chaque frame, on récupère le buffer BGRA/RGBA, on retire le canal alpha, on
//! encode en JPEG et on pousse le résultat dans un canal `watch`.

use crate::protocol::WindowInfo;
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
use tokio::sync::watch;

use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

/// Contrôleur externe : garder vivant, appeler `stop()` pour arrêter.
pub type CaptureSession =
    windows_capture::capture::CaptureControl<CaptureHandler, anyhow::Error>;

#[cfg(windows)]
fn get_process_name(hwnd: isize) -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::System::ProcessStatus::GetModuleBaseNameW;
    use windows::Win32::Foundation::{CloseHandle, HWND};

    unsafe {
        let mut pid = 0;
        let hwnd_raw = HWND(hwnd as *mut core::ffi::c_void);
        GetWindowThreadProcessId(hwnd_raw, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut name = [0u16; 260];
        let len = GetModuleBaseNameW(handle, None, &mut name);
        let _ = CloseHandle(handle);
        if len > 0 {
            Some(String::from_utf16_lossy(&name[..len as usize]))
        } else {
            None
        }
    }
}

pub fn list_windows(locked_apps: &[String]) -> anyhow::Result<Vec<WindowInfo>> {
    let mut out = Vec::new();
    for w in Window::enumerate().map_err(|e| anyhow::anyhow!("enumerate: {e:?}"))? {
        let title = w.title().unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let id = w.as_raw_hwnd() as isize;
        let proc_name = get_process_name(id);
        
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
            id,
            title,
            locked,
        });
    }
    Ok(out)
}

fn downscale_rgba(rgba: &[u8], w: u32, h: u32, scale: f32) -> (Vec<u8>, u32, u32) {
    if scale >= 0.99 {
        return (rgba.to_vec(), w, h);
    }
    let new_w = ((w as f32 * scale) as u32).max(1);
    let new_h = ((h as f32 * scale) as u32).max(1);
    let mut out = Vec::with_capacity((new_w * new_h * 4) as usize);
    for y in 0..new_h {
        let src_y = (((y as f32 / scale) as u32)).min(h - 1);
        let src_row_offset = (src_y * w * 4) as usize;
        for x in 0..new_w {
            let src_x = (((x as f32 / scale) as u32)).min(w - 1);
            let src_pixel_offset = src_row_offset + (src_x * 4) as usize;
            out.push(rgba[src_pixel_offset]);
            out.push(rgba[src_pixel_offset + 1]);
            out.push(rgba[src_pixel_offset + 2]);
            out.push(rgba[src_pixel_offset + 3]);
        }
    }
    (out, new_w, new_h)
}

pub fn start(
    hwnd: isize,
    quality: u8,
    scale: f32,
) -> anyhow::Result<(CaptureSession, watch::Receiver<Vec<u8>>)> {
    let window = Window::from_raw_hwnd(hwnd as *mut core::ffi::c_void);
    let (tx, rx) = watch::channel(Vec::new());

    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        FrameSink { tx, quality, scale },
    );

    let control = CaptureHandler::start_free_threaded(settings)
        .map_err(|e| anyhow::anyhow!("start capture: {e:?}"))?;

    Ok((control, rx))
}

/// Données passées au handler à sa création (`Settings` "flags").
pub struct FrameSink {
    tx: watch::Sender<Vec<u8>>,
    quality: u8,
    scale: f32,
}

pub struct CaptureHandler {
    sink: FrameSink,
}

impl GraphicsCaptureApiHandler for CaptureHandler {
    type Flags = FrameSink;
    type Error = anyhow::Error;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { sink: ctx.flags })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let mut buffer = frame.buffer()?;
        let width = buffer.width();
        let height = buffer.height();
        // Buffer RGBA sans padding de ligne.
        let rgba = buffer.as_nopadding_buffer()?;

        let (scaled_rgba, w, h) = if self.sink.scale < 0.99 {
            downscale_rgba(rgba, width, height, self.sink.scale)
        } else {
            (rgba.to_vec(), width, height)
        };

        // Strip alpha -> RGB (JPEG n'a pas d'alpha).
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for px in scaled_rgba.chunks_exact(4) {
            rgb.push(px[0]);
            rgb.push(px[1]);
            rgb.push(px[2]);
        }

        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, self.sink.quality)
            .encode(&rgb, w, h, ExtendedColorType::Rgb8)?;

        // Écrase la frame précédente ; ignore l'erreur si plus de receiver.
        let _ = self.sink.tx.send(jpeg);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        tracing::info!("capture session closed");
        Ok(())
    }
}

pub fn is_window_locked(hwnd: isize, locked_apps: &[String]) -> bool {
    if locked_apps.is_empty() {
        return false;
    }
    let window = Window::from_raw_hwnd(hwnd as *mut core::ffi::c_void);
    let title = window.title().unwrap_or_default();
    let proc_name = get_process_name(hwnd);
    
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
