//! Messages échangés sur le WebSocket (texte = JSON, binaire = frame JPEG).
//!
//! Les coordonnées souris (`x`, `y`) envoyées par le client sont déjà exprimées
//! dans le **repère natif de la fenêtre** (pixels client), la translation étant
//! faite côté frontend à partir des dimensions renvoyées par `Selected`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Client -> Serveur.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Demande la liste des fenêtres capturables.
    ListWindows,
    /// Force la fenêtre sélectionnée au premier plan.
    FocusWindow,
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
    /// Persiste une macro depuis l'interface humaine.
    SaveMacro {
        context: String,
        name: String,
        window_id: isize,
        steps: Vec<MacroStep>,
    },
    ListMacros,
    LoadMacro { context: String, name: String },
    TestMacro { steps: Vec<MacroStep> },
    SaveWorkflow { name: String, graph: WorkflowGraph },
    ListWorkflows,
    LoadWorkflow { name: String },
    ExecuteWorkflow { name: String },
    TestWorkflow { graph: WorkflowGraph },
    /// Sauvegarde la frame courante en JPEG dans `dir` (défaut si absent/vide).
    Screenshot { dir: Option<String> },
    /// Démarre l'enregistrement d'une séquence d'images dans un sous-dossier
    /// horodaté de `dir`. `fps` plafonne la cadence d'écriture (défaut 10).
    StartRecording { dir: Option<String>, fps: Option<u32> },
    /// Arrête l'enregistrement en cours.
    StopRecording,
    /// Lance une application sur le bureau virtuel.
    LaunchApp { command: String },
    /// Lance l'application de la fenêtre active courante sur le bureau virtuel.
    LaunchActiveOnVirtual,
    /// Ferme l'application d'une fenêtre spécifiée.
    CloseWindow { id: isize },
    /// Initialise la connexion WebRTC.
    WebrtcInit,
    /// Envoie la réponse SDP (Session Description Protocol) du navigateur.
    WebrtcAnswer { sdp: String },
    /// Transmet un candidat ICE réseau.
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Serveur -> Client (messages de contrôle en JSON ; les frames vidéo sont
/// envoyées séparément en binaire ou en flux RTP via WebRTC).
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
    MacroSaved { path: String, name: String },
    MacroList { items: Vec<MacroSummary> },
    MacroLoaded { macro_file: MacroFile },
    MacroPlayed { name: String, steps: usize },
    WorkflowSaved { name: String, path: String },
    WorkflowList { items: Vec<WorkflowSummary> },
    WorkflowLoaded { workflow: WorkflowFile },
    WorkflowStarted { execution_id: String, name: String },
    WorkflowNodeStarted { execution_id: String, node_id: String },
    WorkflowNodeFinished { execution_id: String, node_id: String, output: WorkflowHandle },
    WorkflowFinished { execution_id: String, name: String, nodes_executed: usize },
    WorkflowFailed { execution_id: String, node_id: Option<String>, message: String },
    /// Évènement informatif poussé au superviseur (ex. un agent MCP a pris la
    /// main sur une fenêtre).
    Notice { message: String },
    Error { message: String },
    /// Offre SDP (Session Description Protocol) générée par le serveur pour WebRTC.
    WebrtcOffer { sdp: String },
    /// Candidat ICE réseau généré par le serveur.
    WebrtcIceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
}

#[derive(Debug, Serialize, Clone)]
pub struct WindowInfo {
    pub id: isize,
    pub title: String,
    pub locked: bool,
    pub is_virtual: bool,
    /// Nom du processus, utilisé comme contexte stable de workflow.
    pub context: String,
}

/// Étape persistée d'un workflow. Les coordonnées pointeur sont normalisées.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MacroStep {
    MouseMove { x: f64, y: f64 },
    MouseButton { button: MouseButton, pressed: bool, x: f64, y: f64 },
    Wheel { delta: i32, x: f64, y: f64 },
    Key { code: String, pressed: bool },
    Text { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroFile {
    pub version: u32,
    pub context: String,
    pub name: String,
    /// Identifiant au moment de l'enregistrement (informatif, non durable).
    pub window_id: isize,
    pub steps: Vec<MacroStep>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MacroSummary {
    pub context: String,
    pub name: String,
    pub steps: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGraph {
    pub nodes: HashMap<String, WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFile {
    pub version: u32,
    pub name: String,
    pub graph: WorkflowGraph,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: WorkflowNodeType,
    pub position: WorkflowPosition,
    pub data: WorkflowNodeData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeType { Start, Macro, Loop, Condition, Wait }

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WorkflowPosition { pub x: f64, pub y: f64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkflowNodeData {
    Start(StartNodeData),
    Macro(MacroNodeData),
    Loop(LoopNodeData),
    Condition(ConditionNodeData),
    Wait(WaitNodeData),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartNodeData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub execution_mode: AppExecutionMode,
    #[serde(default)]
    pub focus: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppExecutionMode { #[default] MainDesktop, VirtualDesktop }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MacroNodeData {
    pub macro_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macro_context: Option<String>,
    pub injection_mode: InjectionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionMode { Foreground, Background }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoopNodeData { pub repetitions: u32 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionNodeData { pub expression: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitNodeData { pub duration_ms: u64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEdge {
    pub id: String,
    pub source_node_id: String,
    pub source_handle: WorkflowHandle,
    pub target_node_id: String,
    pub target_handle: WorkflowInputHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowHandle { Next, OnSuccess, OnError, LoopCycle, Out, True, False, WaitDone }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInputHandle { In }

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSummary { pub name: String, pub nodes: usize, pub edges: usize }

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
    DoubleClick,
    /// Appui en (x,y), déplacements interpolés jusqu'à (to_x,to_y), relâche.
    Drag,
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
    /// Destination normalisée [0,1] pour action=drag.
    #[serde(default)]
    pub to_x: Option<f64>,
    #[serde(default)]
    pub to_y: Option<f64>,
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

#[derive(Debug, Deserialize)]
pub struct SaveMacroArgs {
    pub context: String,
    pub name: String,
    pub window_id: isize,
    pub steps: Vec<MacroStep>,
}

#[derive(Debug, Deserialize)]
pub struct PlayMacroArgs {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SaveWorkflowArgs { pub name: String, pub graph: WorkflowGraph }

#[derive(Debug, Deserialize)]
pub struct ExecuteWorkflowArgs { pub name: String }
