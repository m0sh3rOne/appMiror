//! Tampon de logs partagé + couche `tracing` qui l'alimente.
//!
//! La GUI (section « Network Logs ») lit ce tampon pour afficher l'activité
//! réseau : connexions WebSocket (clients), requêtes clients, et échanges MCP.
//! On ne capture que les évènements `tracing` des cibles réseau (`server`,
//! `mcp`, `gui`) — le pipeline de capture/injection n'y apparaît donc pas.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Une ligne de log affichable dans la GUI.
#[derive(Clone)]
pub struct LogEntry {
    /// Heure locale HH:MM:SS.
    pub time: String,
    pub level: Level,
    /// Dernière composante de la cible (ex. `transport`, `server`).
    pub source: String,
    pub message: String,
}

/// Tampon circulaire partagé et borné des logs réseau.
#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<LogEntry>>>,
    cap: usize,
}

impl LogBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(cap.min(64)))),
            cap,
        }
    }

    fn push(&self, entry: LogEntry) {
        let mut buf = self.inner.lock().unwrap();
        while buf.len() >= self.cap {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Clone des entrées pour l'affichage (le plus ancien en premier).
    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.inner.lock().unwrap().iter().cloned().collect()
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

/// Extrait le champ `message` d'un évènement `tracing`.
struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

/// Ne conserve que les cibles réseau pertinentes pour la section Logs.
fn is_network_target(target: &str) -> bool {
    target.starts_with("appmirror_backend") || target.contains("appmirror")
}

/// Dernière composante du target (`appmirror_backend::mcp::transport` -> `transport`).
fn short_source(target: &str) -> String {
    target.rsplit("::").next().unwrap_or(target).to_string()
}

/// Couche `tracing` qui pousse les évènements réseau dans le [`LogBuffer`].
pub struct LogLayer {
    buf: LogBuffer,
}

impl LogLayer {
    pub fn new(buf: LogBuffer) -> Self {
        Self { buf }
    }
}

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if !is_network_target(meta.target()) {
            return;
        }
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        if visitor.0.is_empty() {
            return;
        }
        self.buf.push(LogEntry {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            level: *meta.level(),
            source: short_source(meta.target()),
            message: visitor.0,
        });
    }
}
