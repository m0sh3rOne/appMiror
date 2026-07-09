//! Messages échangés sur le WebSocket (texte = JSON, binaire = frame JPEG).
//!
//! Les coordonnées souris (`x`, `y`) envoyées par le client sont déjà exprimées
//! dans le **repère natif de la fenêtre** (pixels client), la translation étant
//! faite côté frontend à partir des dimensions renvoyées par `Selected`.

use serde::{Deserialize, Serialize};

/// Client -> Serveur.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Demande la liste des fenêtres capturables.
    ListWindows,
    /// Sélectionne la fenêtre à streamer (HWND transporté en `isize`).
    SelectWindow {
        id: isize,
        pin: Option<String>,
        quality: Option<u8>,
        scale: Option<f32>,
        /// Inclure le curseur distant dans la capture (défaut : true).
        show_cursor: Option<bool>,
    },
    /// Bascule le mode d'injection : `background=true` cible le HWND via
    /// `PostMessage` (sans voler le focus ni bouger le curseur physique),
    /// `false` = `SendInput` classique au premier plan.
    SetInputMode { background: bool },
    /// Déplacement souris, coords natives (px client).
    MouseMove { x: i32, y: i32 },
    /// Bouton souris pressé/relâché.
    MouseButton {
        button: MouseButton,
        pressed: bool,
        x: i32,
        y: i32,
    },
    /// Molette. `delta` positif = vers le haut (comme WHEEL_DELTA/120).
    Wheel { delta: i32, x: i32, y: i32 },
    /// Touche clavier. `code` = `KeyboardEvent.code` (ex: "KeyA", "Enter").
    Key { code: String, pressed: bool },
    /// Sauvegarde la frame courante en JPEG dans `dir` (défaut si absent/vide).
    Screenshot { dir: Option<String> },
    /// Démarre l'enregistrement d'une séquence d'images dans un sous-dossier
    /// horodaté de `dir`. `fps` plafonne la cadence d'écriture (défaut 10).
    StartRecording { dir: Option<String>, fps: Option<u32> },
    /// Arrête l'enregistrement en cours.
    StopRecording,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Serveur -> Client (messages de contrôle en JSON ; les frames vidéo sont
/// envoyées séparément en binaire).
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Windows { items: Vec<WindowInfo> },
    /// Confirme la sélection et donne la résolution native (repère cible).
    Selected { width: i32, height: i32 },
    /// Capture unique écrite sur disque.
    ScreenshotSaved { path: String },
    /// Enregistrement de séquence démarré (dossier effectif + fps effectif).
    RecordingStarted { dir: String, fps: u32 },
    /// Enregistrement arrêté ; `frames` = nombre d'images écrites.
    RecordingStopped { dir: String, frames: u64 },
    /// Évènement informatif poussé au superviseur (ex. un agent MCP a pris la
    /// main sur une fenêtre).
    Notice { message: String },
    Error { message: String },
}

#[derive(Debug, Serialize, Clone)]
pub struct WindowInfo {
    pub id: isize,
    pub title: String,
    pub locked: bool,
}

// ============================================================================
// MCP (Model Context Protocol) — enveloppe JSON-RPC 2.0.
//
// Ces types modélisent le dialogue avec un LLM externe via le transport SSE
// (voir `mcp/`). Ils sont **distincts** du protocole WebSocket ci-dessus, qui
// sert la supervision humaine.
// ============================================================================

use serde_json::Value;

/// Requête entrante (le champ `jsonrpc` est accepté puis ignoré). Une requête
/// sans `id` est une *notification* : elle n'attend pas de réponse.
#[derive(Debug, Deserialize)]
pub struct McpRequest {
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Réponse JSON-RPC : `result` **ou** `error`, jamais les deux.
#[derive(Debug, Serialize)]
pub struct McpResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
}

impl McpResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }
    pub fn err(id: Value, error: McpError) -> Self {
        Self { jsonrpc: "2.0", id, result: None, error: Some(error) }
    }
}

#[derive(Debug, Serialize)]
pub struct McpError {
    pub code: i32,
    pub message: String,
}

/// Notification serveur -> client. Réservée aux évènements sortants (ex.
/// `notifications/resources/list_changed`) ; non émise pour l'instant.
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct McpNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Méthodes JSON-RPC reconnues par le serveur MCP.
pub enum McpMethod {
    Initialize,
    /// `notifications/initialized` (notification post-handshake).
    Initialized,
    Ping,
    ToolsList,
    ToolsCall,
    ResourcesList,
    ResourcesRead,
    Other(String),
}

impl McpMethod {
    pub fn parse(method: &str) -> Self {
        match method {
            "initialize" => Self::Initialize,
            "notifications/initialized" => Self::Initialized,
            "ping" => Self::Ping,
            "tools/list" => Self::ToolsList,
            "tools/call" => Self::ToolsCall,
            "resources/list" => Self::ResourcesList,
            "resources/read" => Self::ResourcesRead,
            other => Self::Other(other.to_string()),
        }
    }
}

/// Arguments de l'outil `attach_to_window`. `window_id` (renvoyé par
/// `list_windows`) est la voie supportée ; `pid` est réservé (cf. `mcp`).
#[derive(Debug, Deserialize)]
pub struct AttachArgs {
    #[serde(default)]
    pub window_id: Option<isize>,
    #[serde(default)]
    pub pid: Option<u32>,
}

/// Type d'action de l'outil `inject_input`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectAction {
    MouseMove,
    Button,
    Click,
    Wheel,
    Key,
    Text,
}

/// Arguments de l'outil `inject_input`. Les coordonnées `x`/`y` sont
/// **normalisées** dans [0,1] relativement à la fenêtre attachée.
#[derive(Debug, Deserialize)]
pub struct InjectArgs {
    pub action: InjectAction,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub button: Option<MouseButton>,
    #[serde(default)]
    pub pressed: Option<bool>,
    #[serde(default)]
    pub delta: Option<i32>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}
