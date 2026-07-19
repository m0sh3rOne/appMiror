//! Serveur HTTP + WebSocket (axum). Une connexion = une session (un client,
//! une fenêtre capturée à la fois — suffisant pour le POC).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::Response;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;

use crate::app::AppState;
use crate::capture::{self, CaptureSession};
use crate::input;
use crate::protocol::{ClientMessage, SaveMacroArgs, ServerMessage};
use crate::recorder;

type Sender = Arc<Mutex<SplitSink<WebSocket, Message>>>;

/// État par connexion.
#[derive(Default)]
struct Session {
    capture: Option<CaptureSession>,
    forward: Option<tokio::task::JoinHandle<()>>,
    hwnd: Option<isize>,
    /// Clone du receiver de frames — sert aux captures disque.
    frames: Option<tokio::sync::watch::Receiver<Vec<u8>>>,
    recording: Option<recorder::Recording>,
    /// Injection en arrière-plan (PostMessage) plutôt que SendInput au premier
    /// plan : ne vole pas le focus de l'utilisateur hôte.
    input_bg: bool,
    /// Session WebRTC active pour cette connexion.
    #[cfg(feature = "webrtc")]
    webrtc: Option<crate::webrtc::WebRtcSession>,
}

impl Session {
    async fn stop_capture(&mut self) {
        #[cfg(feature = "webrtc")]
        if let Some(w) = self.webrtc.take() {
            w.stop().await;
        }
        if let Some(rec) = self.recording.take() {
            let (dir, frames) = rec.stop();
            tracing::info!("enregistrement arrêté: {} ({frames} frames)", dir.display());
        }
        if let Some(task) = self.forward.take() {
            task.abort();
        }
        if let Some(control) = self.capture.take() {
            let _ = control.stop();
        }
        self.frames = None;
    }
}

pub async fn ws_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state, addr))
}

async fn handle_socket(socket: WebSocket, state: AppState, addr: SocketAddr) {
    tracing::info!("WebSocket client connected: {addr}");
    let (sink, mut stream) = socket.split();
    let sender: Sender = Arc::new(Mutex::new(sink));
    let mut session = Session {
        input_bg: true,
        ..Default::default()
    };
    // Reçoit les évènements poussés par le serveur MCP (ex. prise de contrôle).
    let mut notices = state.notices.subscribe();

    loop {
        tokio::select! {
            // Flux entrant du client humain.
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(txt))) => match serde_json::from_str::<ClientMessage>(&txt) {
                    Ok(cm) => handle_message(cm, &mut session, &sender, &state).await,
                    Err(e) => tracing::warn!("message invalide: {e}"),
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            // Notifications MCP -> superviseur (n'affecte pas le flux de frames).
            notice = notices.recv() => match notice {
                Ok(message) => send_json(&sender, &ServerMessage::Notice { message }).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
        }
    }

    session.stop_capture().await;
    tracing::info!("WebSocket client disconnected: {addr}");
}

async fn send_json(sender: &Sender, msg: &ServerMessage) {
    if let Ok(txt) = serde_json::to_string(msg) {
        let _ = sender.lock().await.send(Message::Text(txt)).await;
    }
}

async fn handle_message(msg: ClientMessage, session: &mut Session, sender: &Sender, state: &AppState) {
    match msg {
        ClientMessage::ListWindows => {
            let (locked_apps, allow_virtual_desktop) = {
                let cfg_opt = state.config.lock().unwrap();
                if let Some(cfg_mutex) = &*cfg_opt {
                    let cfg = cfg_mutex.lock().unwrap();
                    (cfg.locked_apps.clone(), cfg.allow_virtual_desktop)
                } else {
                    (Vec::new(), false)
                }
            };
            match capture::list_windows(&locked_apps, allow_virtual_desktop) {
                Ok(items) => send_json(sender, &ServerMessage::Windows { items }).await,
                Err(e) => {
                    send_json(sender, &ServerMessage::Error { message: e.to_string() }).await
                }
            }
        }

        ClientMessage::FocusWindow => {
            if let Some(id) = session.hwnd {
                input::focus_window(id);
            }
        }

        ClientMessage::SelectWindow { id, pin, quality, scale, show_cursor } => {
            tracing::info!(
                "stream requested: window {id} (quality={:?}, scale={:?}, cursor={:?})",
                quality, scale, show_cursor
            );
            let is_locked = {
                let cfg_opt = state.config.lock().unwrap();
                if let Some(cfg_mutex) = &*cfg_opt {
                    let locked_apps = cfg_mutex.lock().unwrap().locked_apps.clone();
                    capture::is_window_locked(id, &locked_apps)
                } else {
                    false
                }
            };

            if is_locked {
                let correct = pin.as_ref().map(|p| state.verify_pin(p)).unwrap_or(false);
                if !correct {
                    send_json(sender, &ServerMessage::Error { message: "PIN_REQUIRED".to_string() }).await;
                    return;
                }
            }

            session.stop_capture().await;

            // En mode arrière-plan, on ne vole PAS le focus de l'hôte.
            if !session.input_bg {
                input::focus_window(id);
            }
            let (w, h) = input::client_size(id).unwrap_or((0, 0));
            session.hwnd = Some(id);

            let q = quality.unwrap_or(70);
            let s = scale.unwrap_or(1.0);
            let cursor = show_cursor.unwrap_or(true);
            match capture::start(id, q, s, cursor) {
                Ok((control, mut rx)) => {
                    session.capture = Some(control);
                    session.frames = Some(rx.clone());
                    let sender_fwd = sender.clone();
                    // Tâche de forwarding : latest-frame-wins.
                    session.forward = Some(tokio::spawn(async move {
                        while rx.changed().await.is_ok() {
                            let frame = rx.borrow_and_update().clone();
                            if frame.is_empty() {
                                continue;
                            }
                            if sender_fwd
                                .lock()
                                .await
                                .send(Message::Binary(frame))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }));
                    send_json(sender, &ServerMessage::Selected { width: w, height: h }).await;
                }
                Err(e) => {
                    send_json(sender, &ServerMessage::Error { message: e.to_string() }).await
                }
            }
        }

        ClientMessage::SetInputMode { background } => {
            session.input_bg = background;
            // Passer en avant-plan : on ramène la fenêtre au premier plan pour
            // que SendInput l'atteigne. En arrière-plan, on ne touche rien.
            if !background {
                if let Some(hwnd) = session.hwnd {
                    input::focus_window(hwnd);
                }
            }
            tracing::info!("mode d'injection : {}", if background { "arrière-plan (PostMessage)" } else { "premier plan (SendInput)" });
        }

        ClientMessage::MouseMove { x, y } => {
            if let Some(hwnd) = session.hwnd {
                if session.input_bg {
                    input::mouse_move_bg(hwnd, x, y);
                } else {
                    input::mouse_move(hwnd, x, y);
                }
            }
        }
        ClientMessage::MouseButton { button, pressed, x, y } => {
            if let Some(hwnd) = session.hwnd {
                if session.input_bg {
                    input::mouse_button_bg(hwnd, button, pressed, x, y);
                } else {
                    input::mouse_button(hwnd, button, pressed, x, y);
                }
            }
        }
        ClientMessage::Wheel { delta, x, y } => {
            if let Some(hwnd) = session.hwnd {
                if session.input_bg {
                    input::wheel_bg(hwnd, delta, x, y);
                } else {
                    input::wheel(hwnd, delta, x, y);
                }
            }
        }
        ClientMessage::Key { code, pressed } => {
            if session.input_bg {
                if let Some(hwnd) = session.hwnd {
                    input::key_bg(hwnd, &code, pressed);
                }
            } else {
                input::key(session.hwnd, &code, pressed);
            }
        }

        ClientMessage::SaveMacro { context, name, window_id, steps } => {
            if session.hwnd != Some(window_id) {
                send_json(sender, &ServerMessage::Error { message: "la fenêtre de la macro n'est plus la fenêtre active".into() }).await;
                return;
            }
            match crate::macro_store::save(SaveMacroArgs { context, name: name.clone(), window_id, steps }).await {
                Ok(path) => send_json(sender, &ServerMessage::MacroSaved { path: path.display().to_string(), name }).await,
                Err(message) => send_json(sender, &ServerMessage::Error { message }).await,
            }
        }
        ClientMessage::ListMacros => match crate::macro_store::list().await {
            Ok(items) => send_json(sender, &ServerMessage::MacroList { items }).await,
            Err(message) => send_json(sender, &ServerMessage::Error { message }).await,
        },
        ClientMessage::LoadMacro { context, name } => match crate::macro_store::load_named(&context, &name).await {
            Ok(macro_file) => send_json(sender, &ServerMessage::MacroLoaded { macro_file }).await,
            Err(message) => send_json(sender, &ServerMessage::Error { message }).await,
        },
        ClientMessage::TestMacro { steps } => {
            let Some(hwnd) = session.hwnd else {
                send_json(sender, &ServerMessage::Error { message: "aucune fenêtre streamée".into() }).await;
                return;
            };
            let (width, height) = input::client_size(hwnd).unwrap_or((0, 0));
            let file = crate::protocol::MacroFile { version: 1, context: "test".into(), name: "test".into(), window_id: hwnd, steps };
            match crate::macro_player::play(&file, crate::app::AttachedWindow { id: hwnd, width, height }).await {
                Ok(steps) => send_json(sender, &ServerMessage::MacroPlayed { name: "test".into(), steps }).await,
                Err(message) => send_json(sender, &ServerMessage::Error { message }).await,
            }
        }

        ClientMessage::Screenshot { dir } => {
            let frame = session
                .frames
                .as_ref()
                .map(|rx| rx.borrow().clone())
                .unwrap_or_default();
            if frame.is_empty() {
                send_json(
                    sender,
                    &ServerMessage::Error {
                        message: "aucune frame disponible — sélectionnez une fenêtre d'abord".into(),
                    },
                )
                .await;
                return;
            }
            let result = async {
                let dir = recorder::resolve_dir(dir.as_deref())?;
                recorder::save_screenshot(&dir, &frame).await
            }
            .await;
            match result {
                Ok(path) => {
                    send_json(
                        sender,
                        &ServerMessage::ScreenshotSaved { path: path.display().to_string() },
                    )
                    .await
                }
                Err(e) => {
                    send_json(sender, &ServerMessage::Error { message: e.to_string() }).await
                }
            }
        }

        ClientMessage::StartRecording { dir, fps } => {
            if session.recording.is_some() {
                send_json(
                    sender,
                    &ServerMessage::Error { message: "enregistrement déjà en cours".into() },
                )
                .await;
                return;
            }
            let Some(rx) = session.frames.clone() else {
                send_json(
                    sender,
                    &ServerMessage::Error {
                        message: "aucune capture active — sélectionnez une fenêtre d'abord".into(),
                    },
                )
                .await;
                return;
            };
            let result = recorder::resolve_dir(dir.as_deref())
                .and_then(|base| recorder::start_recording(&base, fps.unwrap_or(10), rx));
            match result {
                Ok((rec, fps)) => {
                    let dir = rec.dir().display().to_string();
                    session.recording = Some(rec);
                    send_json(sender, &ServerMessage::RecordingStarted { dir, fps }).await;
                }
                Err(e) => {
                    send_json(sender, &ServerMessage::Error { message: e.to_string() }).await
                }
            }
        }

        ClientMessage::StopRecording => match session.recording.take() {
            Some(rec) => {
                let (dir, frames) = rec.stop();
                send_json(
                    sender,
                    &ServerMessage::RecordingStopped {
                        dir: dir.display().to_string(),
                        frames,
                    },
                )
                .await;
            }
            None => {
                send_json(
                    sender,
                    &ServerMessage::Error { message: "aucun enregistrement en cours".into() },
                )
                .await
            }
        },
        ClientMessage::LaunchApp { command } => {
            #[cfg(windows)]
            {
                if let Some(_hdesk) = crate::input::desktop::get_or_create_virtual_desktop() {
                    let arguments: Vec<String> = Vec::new();
                    match crate::input::desktop::spawn_on_desktop(&command, &arguments, "WinSta0\\appMirror_Desktop") {
                        Ok(pid) => {
                            send_json(sender, &ServerMessage::Notice { message: format!("Application lancée sur le bureau virtuel (PID: {})", pid) }).await;
                        }
                        Err(e) => {
                            send_json(sender, &ServerMessage::Error { message: format!("Échec du lancement : {}", e) }).await;
                        }
                    }
                } else {
                    send_json(sender, &ServerMessage::Error { message: "Impossible de créer/ouvrir le bureau virtuel".to_string() }).await;
                }
            }
            #[cfg(not(windows))]
            {
                let _ = command;
                send_json(sender, &ServerMessage::Error { message: "Bureau virtuel supporté uniquement sur Windows".to_string() }).await;
            }
        }

        ClientMessage::LaunchActiveOnVirtual => {
            #[cfg(windows)]
            {
                if let Some(hwnd) = session.hwnd {
                    if let Some(exe_path) = crate::input::desktop::get_process_image_path(hwnd) {
                        if let Some(_hdesk) = crate::input::desktop::get_or_create_virtual_desktop() {
                            let arguments: Vec<String> = Vec::new();
                            match crate::input::desktop::spawn_on_desktop(&exe_path, &arguments, "WinSta0\\appMirror_Desktop") {
                                Ok(pid) => {
                                    send_json(sender, &ServerMessage::Notice { 
                                        message: format!("Application lancée sur le bureau virtuel (PID: {})", pid) 
                                    }).await;
                                }
                                Err(e) => {
                                    send_json(sender, &ServerMessage::Error { message: format!("Échec du lancement : {}", e) }).await;
                                }
                            }
                        } else {
                            send_json(sender, &ServerMessage::Error { message: "Impossible de créer/ouvrir le bureau virtuel".to_string() }).await;
                        }
                    } else {
                        send_json(sender, &ServerMessage::Error { message: "Impossible de déterminer le chemin de l'application active".to_string() }).await;
                    }
                } else {
                    send_json(sender, &ServerMessage::Error { message: "Aucune fenêtre active sélectionnée".to_string() }).await;
                }
            }
            #[cfg(not(windows))]
            {
                send_json(sender, &ServerMessage::Error { message: "Bureau virtuel supporté uniquement sur Windows".to_string() }).await;
            }
        }

        ClientMessage::CloseWindow { id } => {
            match crate::capture::close_window(id) {
                Ok(_) => {
                    send_json(sender, &ServerMessage::Notice { message: "Fermeture de la fenêtre demandée".to_string() }).await;
                    let sender_clone = sender.clone();
                    let (locked_apps, allow_virtual_desktop) = {
                        let cfg_opt = state.config.lock().unwrap();
                        if let Some(cfg_mutex) = &*cfg_opt {
                            let cfg = cfg_mutex.lock().unwrap();
                            (cfg.locked_apps.clone(), cfg.allow_virtual_desktop)
                        } else {
                            (Vec::new(), false)
                        }
                    };
                    tokio::spawn(async move {
                        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                        if let Ok(windows) = crate::capture::list_windows(&locked_apps, allow_virtual_desktop) {
                            send_json(&sender_clone, &ServerMessage::Windows { items: windows }).await;
                        }
                    });
                }
                Err(e) => {
                    send_json(sender, &ServerMessage::Error { message: format!("Impossible de fermer la fenêtre : {}", e) }).await;
                }
            }
        }

        #[cfg(feature = "webrtc")]
        ClientMessage::WebrtcInit => {
            let Some(rx) = session.frames.clone() else {
                send_json(
                    sender,
                    &ServerMessage::Error {
                        message: "aucune capture active — sélectionnez une fenêtre d'abord".into(),
                    },
                )
                .await;
                return;
            };

            let sender_clone = sender.clone();
            let on_candidate = move |msg| {
                let sender = sender_clone.clone();
                tokio::spawn(async move {
                    send_json(&sender, &msg).await;
                });
            };

            // Arrête toute session WebRTC existante
            if let Some(w) = session.webrtc.take() {
                w.stop().await;
            }

            match crate::webrtc::WebRtcSession::create(rx, on_candidate).await {
                Ok((webrtc_session, sdp_offer)) => {
                    session.webrtc = Some(webrtc_session);
                    send_json(sender, &ServerMessage::WebrtcOffer { sdp: sdp_offer }).await;
                }
                Err(e) => {
                    send_json(sender, &ServerMessage::Error { message: format!("Échec WebRTC : {}", e) }).await;
                }
            }
        }

        #[cfg(feature = "webrtc")]
        ClientMessage::WebrtcAnswer { sdp } => {
            if let Some(ref w) = session.webrtc {
                if let Err(e) = w.set_remote_answer(&sdp).await {
                    send_json(sender, &ServerMessage::Error { message: format!("Échec SDP answer : {}", e) }).await;
                }
            } else {
                send_json(sender, &ServerMessage::Error { message: "Session WebRTC non initiée".into() }).await;
            }
        }

        #[cfg(feature = "webrtc")]
        ClientMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index } => {
            if let Some(ref w) = session.webrtc {
                if let Err(e) = w.add_remote_candidate(&candidate, sdp_mid, sdp_mline_index).await {
                    tracing::debug!("Erreur ICE candidate : {}", e);
                }
            }
        }

        // Sans la feature `webrtc`, les messages de signaling WebRTC sont acceptés
        // mais inertes (le protocole les conserve pour rester en phase avec le front).
        #[cfg(not(feature = "webrtc"))]
        ClientMessage::WebrtcInit
        | ClientMessage::WebrtcAnswer { .. }
        | ClientMessage::IceCandidate { .. } => {
            send_json(
                sender,
                &ServerMessage::Error { message: "WebRTC not compiled in this build".into() },
            )
            .await;
        }
    }
}
