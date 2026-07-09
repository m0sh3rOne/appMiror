//! Serveur HTTP + WebSocket (axum). Une connexion = une session (un client,
//! une fenêtre capturée à la fois — suffisant pour le POC).

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;

use crate::app::AppState;
use crate::capture::{self, CaptureSession};
use crate::input;
use crate::protocol::{ClientMessage, ServerMessage};
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
}

impl Session {
    fn stop_capture(&mut self) {
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

pub async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (sink, mut stream) = socket.split();
    let sender: Sender = Arc::new(Mutex::new(sink));
    let mut session = Session::default();
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

    session.stop_capture();
    tracing::info!("connexion fermée");
}

async fn send_json(sender: &Sender, msg: &ServerMessage) {
    if let Ok(txt) = serde_json::to_string(msg) {
        let _ = sender.lock().await.send(Message::Text(txt)).await;
    }
}

async fn handle_message(msg: ClientMessage, session: &mut Session, sender: &Sender, state: &AppState) {
    match msg {
        ClientMessage::ListWindows => {
            let locked_apps = {
                let cfg_opt = state.config.lock().unwrap();
                if let Some(cfg_mutex) = &*cfg_opt {
                    cfg_mutex.lock().unwrap().locked_apps.clone()
                } else {
                    Vec::new()
                }
            };
            match capture::list_windows(&locked_apps) {
                Ok(items) => send_json(sender, &ServerMessage::Windows { items }).await,
                Err(e) => {
                    send_json(sender, &ServerMessage::Error { message: e.to_string() }).await
                }
            }
        }

        ClientMessage::SelectWindow { id, pin } => {
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

            session.stop_capture();

            input::focus_window(id);
            let (w, h) = input::client_size(id).unwrap_or((0, 0));
            session.hwnd = Some(id);

            match capture::start(id, 70) {
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

        ClientMessage::MouseMove { x, y } => {
            if let Some(hwnd) = session.hwnd {
                input::mouse_move(hwnd, x, y);
            }
        }
        ClientMessage::MouseButton { button, pressed, x, y } => {
            if let Some(hwnd) = session.hwnd {
                input::mouse_button(hwnd, button, pressed, x, y);
            }
        }
        ClientMessage::Wheel { delta, x, y } => {
            if let Some(hwnd) = session.hwnd {
                input::wheel(hwnd, delta, x, y);
            }
        }
        ClientMessage::Key { code, pressed } => {
            input::key(&code, pressed);
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
    }
}
