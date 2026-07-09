//! appMirror — backend de capture/streaming.

mod app;
mod capture;
mod config;
mod gui;
mod input;
mod mcp;
mod protocol;
mod recorder;
mod server;

use std::sync::{Arc, Mutex};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;

use crate::app::AppState;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,appmirror_backend=debug".into()),
        )
        .init();

    let config = config::AppConfig::load();
    let state = AppState::new();

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
            axum::serve(listener, app).await?;
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
