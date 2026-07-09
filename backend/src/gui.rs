use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use eframe::egui;
use rust_embed::RustEmbed;
use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};

#[derive(RustEmbed)]
#[folder = "../frontend/dist"]
pub struct Assets;

pub async fn static_handler(uri: Uri) -> impl IntoResponse {
    let mut path = uri.path().trim_start_matches('/').to_string();
    if path.is_empty() {
        path = "index.html".to_string();
    }

    match Assets::get(&path) {
        Some(content) => {
            let mime = if path.ends_with(".js") {
                "text/javascript"
            } else if path.ends_with(".css") {
                "text/css"
            } else if path.ends_with(".html") {
                "text/html"
            } else {
                "application/octet-stream"
            };

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .body(Body::from(content.data))
                .unwrap()
        }
        None => {
            // Fallback to index.html for SPA routing
            match Assets::get("index.html") {
                Some(content) => Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html")
                    .body(Body::from(content.data))
                    .unwrap(),
                None => Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("404 Not Found"))
                    .unwrap(),
            }
        }
    }
}

pub struct GuiApp {
    config: Arc<Mutex<crate::config::AppConfig>>,
    server_shutdown: Option<oneshot::Sender<()>>,
    new_locked_app: String,
    pin_input: String,
    launcher_pin: String,
    launcher_unlocked: bool,
    local_ip: String,
    app_state: crate::app::AppState,
    port_input: String,
    status_message: String,
}

impl GuiApp {
    pub fn new(cc: &eframe::CreationContext<'_>, config: crate::config::AppConfig, app_state: crate::app::AppState) -> Self {
        // Set visual styles
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals.dark_mode = true;
        cc.egui_ctx.set_style(style);

        let local_ip = local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "127.0.0.1".to_string());

        let launcher_unlocked = !config.launcher_locked;
        let port_input = config.server_port.to_string();
        let pin_input = config.pin.clone();

        Self {
            config: Arc::new(Mutex::new(config)),
            server_shutdown: None,
            new_locked_app: String::new(),
            pin_input,
            launcher_pin: String::new(),
            launcher_unlocked,
            local_ip,
            app_state,
            port_input,
            status_message: "Server is stopped.".to_string(),
        }
    }

    pub fn start_server(&mut self) {
        if self.server_shutdown.is_some() {
            return;
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        self.server_shutdown = Some(shutdown_tx);

        let app_state = self.app_state.clone();
        let config_mutex = self.config.clone();
        let port = {
            let cfg = config_mutex.lock().unwrap();
            cfg.server_port
        };

        // Inject config Arc into the app_state so server handlers can use it
        self.app_state.set_config(config_mutex);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let app = axum::Router::new()
                    .route("/ws", axum::routing::get(crate::server::ws_handler))
                    .route("/api/mcp/sse", axum::routing::get(crate::mcp::transport::sse_handler))
                    .route("/api/mcp/message", axum::routing::post(crate::mcp::transport::message_handler))
                    .fallback(static_handler)
                    .layer(tower_http::cors::CorsLayer::permissive())
                    .with_state(app_state);

                let addr = format!("0.0.0.0:{}", port);
                if let Ok(listener) = tokio::net::TcpListener::bind(&addr).await {
                    tracing::info!("appMirror backend listening on http://{}", addr);
                    let _ = axum::serve(listener, app)
                        .with_graceful_shutdown(async {
                            let _ = shutdown_rx.await;
                        })
                        .await;
                } else {
                    tracing::error!("Failed to bind to {}", addr);
                }
            });
        });

        self.status_message = format!("Server is running on http://{}:{}", self.local_ip, port);
    }

    fn stop_server(&mut self) {
        if let Some(shutdown_tx) = self.server_shutdown.take() {
            let _ = shutdown_tx.send(());
            self.status_message = "Server stopped.".to_string();
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.launcher_unlocked {
            // Draw Launcher Unlock Screen
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.heading("🔒 appMirror Controller Locked");
                    ui.add_space(20.0);
                    ui.label("Enter PIN code to unlock the interface:");
                    
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.launcher_pin)
                            .password(true)
                            .hint_text("PIN Code")
                            .font(egui::FontId::proportional(20.0))
                    );
                    
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let config = self.config.lock().unwrap();
                        if self.launcher_pin == config.pin {
                            self.launcher_unlocked = true;
                            self.launcher_pin.clear();
                        } else {
                            self.launcher_pin.clear();
                            ui.label("Incorrect PIN. Please try again.");
                        }
                    }
                    
                    ui.add_space(20.0);
                    if ui.button("Unlock").clicked() {
                        let config = self.config.lock().unwrap();
                        if self.launcher_pin == config.pin {
                            self.launcher_unlocked = true;
                            self.launcher_pin.clear();
                        } else {
                            self.launcher_pin.clear();
                        }
                    }
                });
            });
            return;
        }

        // Draw Main Control Panel
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("appMirror Manager & Controller");
            ui.horizontal(|ui| {
                ui.label("Server Status:");
                let is_running = self.server_shutdown.is_some();
                if is_running {
                    ui.colored_label(egui::Color32::from_rgb(46, 204, 113), "● Running");
                } else {
                    ui.colored_label(egui::Color32::from_rgb(231, 76, 60), "○ Stopped");
                }
            });

            ui.add_space(10.0);

            // Server actions
            ui.horizontal(|ui| {
                if self.server_shutdown.is_none() {
                    if ui.button("▶ Start Server").clicked() {
                        self.start_server();
                    }
                } else {
                    if ui.button("⏹ Stop Server").clicked() {
                        self.stop_server();
                    }
                }

                if ui.button("🌐 Open Web Client").clicked() {
                    let port = self.config.lock().unwrap().server_port;
                    let url = format!("http://localhost:{}", port);
                    let _ = webbrowser::open(&url);
                }
            });

            ui.label(&self.status_message);

            ui.separator();

            // Display IP addresses and port
            let port = self.config.lock().unwrap().server_port;
            ui.group(|ui| {
                ui.heading("Connection Info");
                ui.horizontal(|ui| {
                    ui.label("Localhost Address:");
                    ui.code(format!("http://localhost:{}", port));
                });
                ui.horizontal(|ui| {
                    ui.label("LAN Address (for tablets/phones):");
                    ui.code(format!("http://{}:{}", self.local_ip, port));
                });
                ui.horizontal(|ui| {
                    ui.label("MCP Server Address:");
                    ui.code(format!("http://localhost:{}/api/mcp/sse", port));
                });
            });

            ui.add_space(10.0);

            // Tabs for settings and manual
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.collapsing("⚙ Settings & Security Locks", |ui| {
                    let mut config_changed = false;
                    let mut config = self.config.lock().unwrap().clone();

                    ui.horizontal(|ui| {
                        ui.label("Server Port:");
                        if ui.add(egui::TextEdit::singleline(&mut self.port_input).desired_width(60.0)).changed() {
                            if let Ok(new_port) = self.port_input.parse::<u16>() {
                                config.server_port = new_port;
                                config_changed = true;
                            }
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Security PIN Code (XXXX):");
                        if ui.add(egui::TextEdit::singleline(&mut self.pin_input).desired_width(80.0)).changed() {
                            config.pin = self.pin_input.clone();
                            config_changed = true;
                        }
                    });

                    if ui.checkbox(&mut config.launcher_locked, "Require PIN to open launcher GUI").changed() {
                        config_changed = true;
                    }

                    ui.separator();
                    ui.label("Locked Applications (Window titles or process names):");
                    
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.new_locked_app).hint_text("e.g. notepad.exe or Calculator"));
                        if ui.button("Lock").clicked() && !self.new_locked_app.trim().is_empty() {
                            config.locked_apps.push(self.new_locked_app.trim().to_string());
                            self.new_locked_app.clear();
                            config_changed = true;
                        }
                    });

                    let mut to_remove = None;
                    for (idx, app) in config.locked_apps.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(format!("🔒 {}", app));
                            if ui.button("🗑").clicked() {
                                to_remove = Some(idx);
                            }
                        });
                    }
                    if let Some(idx) = to_remove {
                        config.locked_apps.remove(idx);
                        config_changed = true;
                    }

                    if config_changed {
                        let _ = config.save();
                        *self.config.lock().unwrap() = config;
                        // update running server config state
                        self.app_state.set_config(self.config.clone());
                    }
                });

                ui.collapsing("📖 User Manual", |ui| {
                    ui.group(|ui| {
                        ui.heading("appMirror User Manual");
                        ui.add_space(5.0);
                        ui.label("appMirror streams a single desktop window to your web browser with ultra-low latency. You can interact with the streamed application using mouse and keyboard inputs directly from the web client.");
                        
                        ui.add_space(10.0);
                        ui.heading("How to Use:");
                        ui.label("1. Click the 'Start Server' button in this controller.");
                        ui.label("2. Open the web client by clicking 'Open Web Client' or navigating to the Localhost / LAN address on another device (e.g. tablet or phone).");
                        ui.label("3. Select the window you want to stream from the drop-down menu in the web browser.");
                        ui.label("4. Click 'Streamer' to launch the live stream and enable remote control.");

                        ui.add_space(10.0);
                        ui.heading("Process Locking & PIN Security:");
                        ui.label("- You can restrict access to sensitive applications (like Notepad, browser tabs, or command line consoles).");
                        ui.label("- Add the window title or process name (e.g. 'notepad.exe') to the Settings list above.");
                        ui.label("- When an application is locked, any browser attempting to select/stream it will be prompted to enter the Security PIN.");
                        ui.label("- You can configure the Security PIN under the Settings panel.");

                        ui.add_space(10.0);
                        ui.heading("MCP Server Integration (for AI tools):");
                        ui.label("- appMirror includes an MCP (Model Context Protocol) server.");
                        ui.label("- You can connect AI agents (e.g. Claude Desktop, Cursor) to appMirror using the MCP Server Address displayed in the Connection Info section.");
                        ui.label("- The AI can list windows, attach to a window, and inject keyboard/mouse inputs natively.");
                    });
                });
            });
        });
    }
}
