#![windows_subsystem = "windows"]
//! appMirror — backend de capture/streaming.

mod app;
mod capture;
mod config;
mod gui;
mod input;
mod logbuf;
mod macro_player;
mod macro_store;
mod mcp;
mod protocol;
mod recorder;
mod server;
#[cfg(feature = "webrtc")]
mod h264;
#[cfg(feature = "webrtc")]
mod webrtc;


use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tracing_subscriber::prelude::*;

use crate::app::AppState;
use crate::logbuf::{LogBuffer, LogLayer};

fn main() -> anyhow::Result<()> {
    // Tampon partagé alimenté par une couche `tracing` dédiée, en plus de la
    // sortie stdout habituelle. La GUI l'affiche dans sa section « Network Logs ».
    let logs = LogBuffer::new(500);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,appmirror_backend=debug".into());
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(LogLayer::new(logs.clone()))
        .init();

    let config = config::AppConfig::load();
    let state = AppState::new(logs);

    // Check for --headless flag
    let args: Vec<String> = std::env::args().collect();
    let headless = args.iter().any(|arg| arg == "--headless");

    if headless {
        // Run headlessly in the main thread (tokio)
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config_arc = Arc::new(Mutex::new(config.clone()));
            state.set_config(config_arc);

            let app = Router::new()
                .route("/ws", get(server::ws_handler))
                .route("/api/mcp/sse", get(mcp::transport::sse_handler).post(mcp::transport::message_handler))
                .route("/api/mcp/message", post(mcp::transport::message_handler))
                .fallback(gui::static_handler)
                .layer(CorsLayer::permissive())
                .with_state(state);

            let addr = format!("0.0.0.0:{}", config.server_port);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            tracing::info!("appMirror backend (headless) listening on http://{} (ws: /ws)", addr);
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        })?;
    } else {
        // Run with eframe GUI
        let native_options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_title("appMirror Controller")
                .with_inner_size([550.0, 520.0]),
            ..Default::default()
        };

        let _ = eframe::run_native(
            "appMirror Controller",
            native_options,
            Box::new(move |cc| {
                let mut app = gui::GuiApp::new(cc, config, state);
                // Start the server automatically on startup
                app.start_server();
                Box::new(app)
            }),
        );
    }

    Ok(())
}
