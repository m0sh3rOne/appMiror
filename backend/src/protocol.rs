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
    /// Texte Unicode déjà interprété selon le layout du navigateur.
    TextInput { text: String },
    /// Persiste une macro depuis l'interface humaine.
    SaveMacro {
        context: String,
        name: String,
        window_id: isize,
        #[serde(default)]
        params: Vec<MacroParam>,
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
    /// Persistance des plans UI dessinés dans le module Layout.
    SaveLayout {
        context: String,
        name: String,
        source_width: u32,
        source_height: u32,
        zones: Vec<LayoutZone>,
    },
    ListLayouts { context: Option<String> },
    LoadLayout { context: String, name: String },
    /// Capture l'état courant : hash de chaque zone sur la dernière frame.
    CaptureState {
        context: String,
        name: String,
        layout: Option<String>,
        zones: Vec<LayoutZone>,
    },
    ListStates { context: Option<String> },
    DeleteState { context: String, name: String },
    SaveProfile { profile: AppProfile },
    GetProfile { context: String },
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
    Selected { width: i32, height: i32, background: bool },
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
    LayoutSaved { context: String, name: String, path: String },
    LayoutList { items: Vec<LayoutSummary> },
    LayoutLoaded { layout: LayoutFile },
    StateSaved { context: String, name: String, signatures: usize },
    StateList { items: Vec<UiStateSummary> },
    StateDeleted { context: String, name: String },
    ProfileSaved { context: String },
    ProfileLoaded { profile: AppProfile },
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
    /// Cible synthétique représentant le bureau complet plutôt qu'une fenêtre.
    #[serde(default)]
    pub is_desktop: bool,
    /// Nom du processus, utilisé comme contexte stable de workflow.
    pub context: String,
}

/// Étape persistée d'un workflow. Les coordonnées pointeur sont normalisées.
/// Les étapes brutes (souris/clavier) sont complétées d'étapes sémantiques qui
/// référencent les layouts/états du contexte — les champs texte y acceptent la
/// substitution de paramètres `{{nom}}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MacroStep {
    MouseMove { x: f64, y: f64 },
    MouseButton { button: MouseButton, pressed: bool, x: f64, y: f64 },
    Wheel { delta: i32, x: f64, y: f64 },
    Key { code: String, pressed: bool },
    Text { text: String },
    /// Active un élément de layout avec stratégies de repli + vérification.
    ActivateElement {
        element: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<String>,
        /// auto | click | click_then_enter | double_click (défaut auto).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strategy: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verify: Option<bool>,
        /// Autorité de validation; par défaut `state` si la zone déclare un
        /// expectedState, sinon `screen_change`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verification: Option<Verification>,
    },
    /// Clique sur un élément puis y tape le texte (substituable).
    TypeIntoElement {
        element: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<String>,
    },
    /// Attend qu'un état enregistré soit détecté à l'écran.
    WaitForState {
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    /// Pause fixe.
    WaitMs { duration_ms: u64 },
    /// Exécute une autre macro (profondeur limitée), avec les mêmes paramètres.
    CallMacro {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
    },
}

/// Paramètre déclaré d'une macro, substitué via `{{nom}}` dans les étapes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacroParam {
    pub name: String,
    pub kind: MacroParamKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Valeurs admises pour kind=choice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroParamKind {
    Text,
    Number,
    Boolean,
    /// Jamais journalisé ni renvoyé dans les rapports.
    Secret,
    Choice,
    /// Référence à une zone de layout (id/nom/label).
    ZoneRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroFile {
    pub version: u32,
    pub context: String,
    pub name: String,
    /// Identifiant au moment de l'enregistrement (informatif, non durable).
    pub window_id: isize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<MacroParam>,
    pub steps: Vec<MacroStep>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MacroSummary {
    pub context: String,
    pub name: String,
    pub steps: usize,
}

/// Plan d'interface défini par l'utilisateur pour un contexte applicatif.
/// Les limites sont normalisées afin de rester valides après redimensionnement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutFile {
    pub version: u32,
    pub context: String,
    pub name: String,
    pub source_width: u32,
    pub source_height: u32,
    pub zones: Vec<LayoutZone>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutZone {
    pub id: String,
    pub name: String,
    pub kind: LayoutZoneKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub bounds: NormalizedBounds,
    /// Texte visible attendu à l'écran (ex. "New Game"), distinct du nom interne.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Séquence d'activation à rejouer dans l'ordre; None = stratégie auto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<Vec<ActivationStep>>,
    /// État applicatif attendu après activation réussie (ex. "gameplay").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_state: Option<String>,
    /// Position définie relativement à une autre zone plutôt qu'en absolu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<ZoneAnchor>,
}

/// Étape d'activation d'une zone. `TypeText` consomme le texte fourni à
/// l'exécution (type_into_ui_element); les autres étapes sont autonomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ActivationStep {
    // Note: `rename_all = "snake_case"` ne s'applique qu'aux *variantes*; les
    // champs ont besoin de `rename_all_fields` pour suivre le camelCase du
    // reste du protocole (LayoutZone, ZoneAnchor…). Sans cela `holdMs` et
    // `delayAfterMs` étaient silencieusement ignorés.
    /// `click` est un alias : les deux nomment le clic au centre de la zone.
    #[serde(alias = "click")]
    ClickCenter {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_after_ms: Option<u64>,
    },
    DoubleClick {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_after_ms: Option<u64>,
    },
    Key {
        code: String,
        /// Durée de maintien de la touche; certaines applications ignorent un
        /// appui/relâche instantané.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_after_ms: Option<u64>,
    },
    FocusThenKey {
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_after_ms: Option<u64>,
    },
    TypeText {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_after_ms: Option<u64>,
    },
    /// Attend un état enregistré : rend la séquence auto-vérifiante.
    WaitForState {
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    WaitMs { duration_ms: u64 },
}

impl ActivationStep {
    /// Délai à respecter après l'étape, avant la suivante.
    pub fn delay_after_ms(&self) -> Option<u64> {
        match self {
            Self::ClickCenter { delay_after_ms }
            | Self::DoubleClick { delay_after_ms }
            | Self::TypeText { delay_after_ms }
            | Self::Key { delay_after_ms, .. }
            | Self::FocusThenKey { delay_after_ms, .. } => *delay_after_ms,
            Self::WaitForState { .. } | Self::WaitMs { .. } => None,
        }
    }
}

/// Autorité de validation d'une activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    /// Seule la détection de `expectedState` vaut succès (aucun repli).
    State,
    /// Un changement d'écran suffit — vérification faible, sujette aux faux
    /// positifs (un simple surlignage compte comme un changement).
    ScreenChange,
    /// Aucune vérification : la première séquence est jouée puis on rend la main.
    None,
}

/// Ancrage relatif: les bounds de la zone sont décalés (dx, dy) — en
/// coordonnées normalisées — par rapport aux bounds de la zone `relative_to`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneAnchor {
    pub relative_to: String,
    #[serde(default)]
    pub dx: f64,
    #[serde(default)]
    pub dy: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutZoneKind {
    Button,
    Input,
    Textarea,
    Dropdown,
    Menu,
    Checkbox,
    Radio,
    Link,
    Text,
    Region,
    /// Interrupteur à deux états (distinct d'une case à cocher).
    Toggle,
    /// Curseur de valeur continue.
    Slider,
    /// Incrémenteur (±) à valeur discrète.
    Stepper,
    /// Onglet d'un groupe d'onglets.
    Tab,
    /// Bouton de validation/annulation d'une boîte de dialogue.
    DialogAction,
}

impl LayoutZoneKind {
    /// Valeurs exposées dans le JSON Schema MCP — doit rester exhaustif.
    pub const ALL: &'static [&'static str] = &[
        "button", "input", "textarea", "dropdown", "menu", "checkbox", "radio",
        "link", "text", "region", "toggle", "slider", "stepper", "tab", "dialog_action",
    ];
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutSummary {
    pub context: String,
    pub name: String,
    pub zones: usize,
}

/// État applicatif nommé (ex. "main_menu", "gameplay"), reconnu par les
/// signatures visuelles de zones capturées sur une frame de référence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStateFile {
    pub version: u32,
    pub context: String,
    pub name: String,
    /// Layout à utiliser quand cet état est détecté.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    pub signatures: Vec<StateSignature>,
}

/// Empreinte d'une zone : dHash 8×8 (hexadécimal) + luminance moyenne du crop.
/// Les bounds sont recopiés du layout au moment de la capture pour que la
/// détection fonctionne sans recharger le layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateSignature {
    pub zone_id: String,
    pub bounds: NormalizedBounds,
    pub dhash: String,
    pub mean_luma: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStateSummary {
    pub context: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    pub signatures: usize,
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
pub enum WorkflowNodeType { Start, Macro, Loop, Condition, Wait, Activate, WaitForState }

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
    Activate(ActivateNodeData),
    WaitForState(WaitForStateNodeData),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartNodeData {
    /// Nature explicite de la cible. Optionnel pour relire les workflows v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<StartTargetKind>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartTargetKind { Window, Command, MainDesktop, VirtualDesktop }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MacroNodeData {
    pub macro_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macro_context: Option<String>,
    pub injection_mode: InjectionMode,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parameters: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub verify: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivateNodeData {
    pub element: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_state: Option<String>,
    #[serde(default = "default_true")]
    pub verify: bool,
    /// Autorité de validation; par défaut déduite de la zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    #[serde(default = "default_5000")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub retries: u32,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitForStateNodeData {
    pub state: String,
    #[serde(default = "default_5000")]
    pub timeout_ms: u64,
}

fn default_5000() -> u64 {
    5000
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Running,
    Finished,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeExecutionLog {
    pub node_id: String,
    pub node_type: String,
    pub started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_attempted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRecord {
    pub execution_id: String,
    pub workflow_name: String,
    pub status: ExecutionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_node_id: Option<String>,
    pub started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
    pub nodes_executed: usize,
    pub history: Vec<NodeExecutionLog>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppProfile {
    pub context: String,
    #[serde(default)]
    pub foreground_required: bool,
    #[serde(default = "default_auto")]
    pub preferred_activation: String,
    #[serde(default = "default_supported")]
    pub background_mouse: String,
    #[serde(default = "default_supported")]
    pub background_keyboard: String,
    #[serde(default)]
    pub pause_on_focus_loss: bool,
    /// Modèle de sélection de l'interface : `pointer` (le clic active
    /// directement), `keyboard` (une sélection interne se déplace aux flèches
    /// et Entrée valide — cas des menus de jeu, où le survol souris et la
    /// sélection clavier peuvent diverger), `hybrid`, ou `unknown`.
    #[serde(default = "default_unknown")]
    pub selection_model: String,
    /// Touches de navigation quand `selection_model = keyboard`.
    #[serde(default = "default_arrow_down")]
    pub nav_next_key: String,
    #[serde(default = "default_arrow_up")]
    pub nav_prev_key: String,
    /// Délais par défaut de l'application, utilisés quand une étape
    /// d'activation ne fixe pas les siens (cascade étape → profil → constante).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_post_click_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_key_hold_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_post_action_delay_ms: Option<u64>,
}

fn default_auto() -> String { "auto".to_string() }
fn default_supported() -> String { "supported".to_string() }
fn default_unknown() -> String { "unknown".to_string() }
fn default_arrow_down() -> String { "ArrowDown".to_string() }
fn default_arrow_up() -> String { "ArrowUp".to_string() }

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
    #[serde(default)]
    pub params: Vec<MacroParam>,
    pub steps: Vec<MacroStep>,
}

#[derive(Debug, Deserialize)]
pub struct PlayMacroArgs {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SaveWorkflowArgs { pub name: String, pub graph: WorkflowGraph }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteWorkflowArgs {
    pub name: String,
    #[serde(default)]
    pub observe: bool,
    #[serde(default)]
    #[allow(dead_code)]
    pub parameters: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetExecutionStatusArgs {
    pub execution_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelExecutionArgs {
    pub execution_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveLayoutArgs {
    pub context: String,
    pub name: String,
    pub source_width: u32,
    pub source_height: u32,
    pub zones: Vec<LayoutZone>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListLayoutsArgs {
    #[serde(default)]
    pub context: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct GetLayoutArgs {
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

// Les arguments d'outils MCP sont en snake_case (cf. window_id, timeout_ms…),
// comme le publie le JSON Schema. Le camelCase reste accepté en alias : le
// `rename_all = "camelCase"` précédent faisait ignorer *silencieusement* tous
// les champs envoyés selon le schéma, si bien que set_app_profile n'écrivait
// jamais rien.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SetAppProfileArgs {
    pub context: String,
    #[serde(default, alias = "foregroundRequired")]
    pub foreground_required: Option<bool>,
    #[serde(default, alias = "preferredActivation")]
    pub preferred_activation: Option<String>,
    #[serde(default, alias = "backgroundMouse")]
    pub background_mouse: Option<String>,
    #[serde(default, alias = "backgroundKeyboard")]
    pub background_keyboard: Option<String>,
    #[serde(default, alias = "pauseOnFocusLoss")]
    pub pause_on_focus_loss: Option<bool>,
    #[serde(default, alias = "selectionModel")]
    pub selection_model: Option<String>,
    #[serde(default, alias = "navNextKey")]
    pub nav_next_key: Option<String>,
    #[serde(default, alias = "navPrevKey")]
    pub nav_prev_key: Option<String>,
    #[serde(default, alias = "defaultPostClickDelayMs")]
    pub default_post_click_delay_ms: Option<u64>,
    #[serde(default, alias = "defaultKeyHoldMs")]
    pub default_key_hold_ms: Option<u64>,
    #[serde(default, alias = "defaultPostActionDelayMs")]
    pub default_post_action_delay_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAppProfileArgs {
    pub context: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct StepWorkflowArgs {
    pub execution_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeCapabilitiesArgs {
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub window_id: Option<isize>,
}
