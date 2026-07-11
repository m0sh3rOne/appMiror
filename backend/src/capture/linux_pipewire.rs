#![cfg(target_os = "linux")]

//! Capture PipeWire/Wayland sous Linux.
//!
//! Utilise `ashpd` pour communiquer avec le portail `xdg-desktop-portal` (ScreenCast)
//! via DBus pour obtenir un flux PipeWire, puis la crate `pipewire` pour recevoir les
//! frames vidéo, les convertir et les pousser dans la watch-channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::os::unix::io::RawFd;

use ashpd::desktop::screencast::{CursorMode, PersistMode, ScreenCast, SourceType};
use ashpd::WindowIdentifier;
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
use pipewire as pw;
use tokio::sync::watch;

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

pub fn start(
    hwnd: isize,
    quality: u8,
    scale: f32,
    show_cursor: bool,
) -> anyhow::Result<(CaptureSession, watch::Receiver<Vec<u8>>)> {
    let (tx, rx) = watch::channel(Vec::new());
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    // On lance le thread de boucle PipeWire + DBus de manière asynchrone
    let thread = std::thread::spawn(move || {
        if let Err(e) = run_pipewire_capture_loop(hwnd, quality, scale, show_cursor, tx, stop_thread) {
            tracing::error!("Erreur dans le pipeline PipeWire : {:?}", e);
        }
    });

    Ok((CaptureSession { stop, thread: Some(thread) }, rx))
}

fn run_pipewire_capture_loop(
    _hwnd: isize,
    quality: u8,
    scale: f32,
    show_cursor: bool,
    tx: watch::Sender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    // Initialise le runtime Tokio local pour les appels DBus via ashpd
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        // 1. Initialise la connexion au portail ScreenCast
        let screencast = ScreenCast::new().await?;
        let session = screencast.create_session().await?;
        
        let cursor_mode = if show_cursor {
            CursorMode::Embedded
        } else {
            CursorMode::Hidden
        };

        // Sélectionne les sources : demande de capturer une fenêtre ou tout l'écran
        screencast
            .select_sources(
                &session,
                cursor_mode,
                SourceType::Window | SourceType::Monitor,
                false, // multiple sources
                PersistMode::DoNot,
            )
            .await?;

        // Démarre la session de capture pour obtenir le descripteur de fichier PipeWire
        let identifier = WindowIdentifier::default();
        let (streams, fd) = screencast.start(&session, &identifier).await?;
        
        let Some(stream_info) = streams.first() else {
            anyhow::bail!("Aucun flux retourné par le portail ScreenCast");
        };

        let node_id = stream_info.pipewire_node_id();

        // 2. Initialise et lance la boucle de traitement PipeWire
        run_pw_stream(fd, node_id, quality, scale, tx, stop)?;

        Ok(())
    })
}

// Représentation des données transmises aux callbacks PipeWire
struct UserData {
    quality: u8,
    scale: f32,
    tx: watch::Sender<Vec<u8>>,
    stop: Arc<AtomicBool>,
}

fn run_pw_stream(
    fd: RawFd,
    node_id: u32,
    quality: u8,
    scale: f32,
    tx: watch::Sender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    pw::init();

    let main_loop = pw::MainLoop::new(None)?;
    let context = pw::Context::new(&main_loop)?;
    
    // Connecte en réutilisant le descripteur de fichier fourni par le portail
    let properties = pw::properties! {
        *pw::keys::PIPEWIRE_FD => fd.to_string(),
    };
    let core = context.connect(Some(properties))?;

    let user_data = UserData {
        quality,
        scale,
        tx,
        stop: stop.clone(),
    };

    // On configure le listener de flux PipeWire
    let mut stream_listener = pw::stream::StreamListener::new();
    
    // Callback appelé à chaque nouvelle frame disponible
    stream_listener.set_process(move |stream, data: &mut UserData| {
        if data.stop.load(Ordering::Relaxed) {
            let _ = stream.disconnect();
            return;
        }

        // Récupère le buffer de la frame
        let mut buffer = match stream.dequeue_buffer() {
            Some(b) => b,
            None => return,
        };

        let chunk = buffer.datas_mut().first_mut().and_then(|d| d.chunk_mut());
        let size = chunk.map(|c| c.size()).unwrap_or(0);
        let offset = chunk.map(|c| c.offset()).unwrap_or(0);

        if size > 0 {
            // Accède aux données brutes du pixel buffer
            let data_ptr = buffer.datas().first().and_then(|d| d.data());
            if let Some(slice) = data_ptr {
                let frame_data = &slice[(offset as usize)..((offset + size) as usize)];
                
                // Ici, nous supposons un format standard RGB/RGBA du buffer PipeWire.
                // Dans une implémentation complète, on inspecte stream.format() (spa_video_info)
                // pour gérer les conversions YUV, DMA-BUF, BGRA, etc.
                let width = 1280; // Placeholder, extrait du format négocié
                let height = 720; // Placeholder, extrait du format négocié
                
                if let Err(e) = process_and_send_frame(frame_data, width, height, data.quality, data.scale, &data.tx) {
                    tracing::warn!("Échec du traitement de la frame PipeWire : {:?}", e);
                }
            }
        }

        let _ = stream.queue_buffer(buffer);
    });

    // Initialise le flux sur le noeud PipeWire spécifié par le portail
    let mut stream = pw::stream::Stream::new(
        &core,
        "appMirror ScreenCast",
        pw::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let _listener = stream.add_listener_with_user_data(user_data, stream_listener)?;

    // Négocie le format vidéo (RGB/RGBA supportés)
    let mut params = [
        pw::spa::pod::Pod::from_bytes(&[]) // En pratique, configure les formats vidéo valides SPA (Raw Video)
    ];

    stream.connect(
        pw::spa::sys::SPA_DIRECTION_INPUT,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    // Fait tourner la boucle d'événements jusqu'à ce que la capture soit arrêtée
    while !stop.load(Ordering::Relaxed) {
        main_loop.iterate(true);
    }

    Ok(())
}

fn process_and_send_frame(
    raw_data: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    scale: f32,
    tx: &watch::Sender<Vec<u8>>,
) -> anyhow::Result<()> {
    // Si la frame fait moins de pixels que prévu, ignore
    if raw_data.len() < (width * height * 3) as usize {
        anyhow::bail!("Buffer trop petit pour la résolution spécifiée");
    }

    // Supposons une entrée BGRx/RGBA, nous faisons la conversion vers RGB
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    let chunk_size = if raw_data.len() >= (width * height * 4) as usize { 4 } else { 3 };

    for px in raw_data.chunks_exact(chunk_size).take((width * height) as usize) {
        if chunk_size == 4 {
            // BGRA -> RGB
            rgb.push(px[2]);
            rgb.push(px[1]);
            rgb.push(px[0]);
        } else {
            // BGR -> RGB
            rgb.push(px[2]);
            rgb.push(px[1]);
            rgb.push(px[0]);
        }
    }

    let (scaled_rgb, final_w, final_h) = if scale < 0.99 {
        let new_w = ((width as f32 * scale) as u32).max(1);
        let new_h = ((height as f32 * scale) as u32).max(1);
        let mut out = Vec::with_capacity((new_w * new_h * 3) as usize);
        for y in 0..new_h {
            let src_y = (((y as f32 / scale) as u32)).min(height - 1);
            let src_row_offset = (src_y * width * 3) as usize;
            for x in 0..new_w {
                let src_x = (((x as f32 / scale) as u32)).min(width - 1);
                let src_pixel_offset = src_row_offset + (src_x * 3) as usize;
                out.push(rgb[src_pixel_offset]);
                out.push(rgb[src_pixel_offset + 1]);
                out.push(rgb[src_pixel_offset + 2]);
            }
        }
        (out, new_w, new_h)
    } else {
        (rgb, width, height)
    };

    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, quality)
        .encode(&scaled_rgb, final_w, final_h, ExtendedColorType::Rgb8)?;

    let _ = tx.send(jpeg);
    Ok(())
}
