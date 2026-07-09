//! État applicatif partagé entre les routes Axum.
//!
//! Volontairement additif : le streaming WebSocket garde son modèle « une
//! connexion = une session » (voir `server.rs`). Cet état ajoute seulement ce
//! dont le serveur MCP a besoin — la fenêtre ciblée par l'agent et les canaux
//! de liaison — sans interférer avec le pipeline de frames.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};

/// Fenêtre actuellement ciblée par l'agent MCP pour l'injection d'entrées.
/// `width`/`height` (px client) servent à dénormaliser les coordonnées [0,1].
#[derive(Clone, Copy, Debug)]
pub struct AttachedWindow {
    pub id: isize,
    pub width: i32,
    pub height: i32,
}

/// Registre des sessions SSE MCP : `sessionId` -> canal vers le flux descendant.
/// Le message porté est une charge JSON-RPC déjà sérialisée.
pub type McpSessions = Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>;

#[derive(Clone)]
pub struct AppState {
    /// Fenêtre ciblée par l'agent MCP (None tant qu'aucun `attach_to_window`).
    pub attached: Arc<Mutex<Option<AttachedWindow>>>,
    /// Diffusion d'évènements MCP vers les superviseurs humains (WebSocket).
    pub notices: broadcast::Sender<String>,
    /// Sessions SSE MCP ouvertes.
    pub mcp_sessions: McpSessions,
    /// Configuration pour la sécurité PIN / processus verrouillés.
    pub config: Arc<Mutex<Option<Arc<Mutex<crate::config::AppConfig>>>>>,
}

impl AppState {
    pub fn new() -> Self {
        // On garde uniquement l'émetteur ; les WebSocket s'abonnent à la demande.
        let (notices, _) = broadcast::channel(32);
        Self {
            attached: Arc::new(Mutex::new(None)),
            notices,
            mcp_sessions: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_config(&self, config: Arc<Mutex<crate::config::AppConfig>>) {
        *self.config.lock().unwrap() = Some(config);
    }



    pub fn verify_pin(&self, pin: &str) -> bool {
        let cfg_opt = self.config.lock().unwrap().clone();
        if let Some(cfg_mutex) = cfg_opt {
            let cfg = cfg_mutex.lock().unwrap();
            return cfg.pin == pin;
        }
        true
    }
}
