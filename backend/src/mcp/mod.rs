//! Moteur MCP : routeur JSON-RPC 2.0 et implémentation des outils/ressources.
//!
//! Isolé du serveur WebSocket de supervision humaine (`server.rs`). Les outils
//! s'appuient sur les mêmes façades multiplateformes `capture` / `input` que le
//! reste du backend. La couche réseau (SSE + POST) est dans `transport.rs`.
//!
//! Convention MCP respectée : une erreur *protocolaire* (méthode/param invalide)
//! renvoie un `error` JSON-RPC ; un échec *d'exécution d'outil* renvoie un
//! `result` avec `isError: true` — le LLM peut ainsi le lire et réagir.

pub mod transport;

use std::path::Path;

use base64::Engine;
use serde_json::{json, Value};

use crate::app::{AppState, AttachedWindow};
use crate::protocol::{
    AttachArgs, InjectAction, InjectArgs, McpError, McpMethod, McpRequest, McpResponse, MouseButton,
};
use crate::{capture, input, recorder};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Route une requête JSON-RPC. Renvoie `None` pour les notifications (pas de
/// réponse attendue), sinon la `McpResponse` à renvoyer au client.
pub async fn handle_mcp_request(req: McpRequest, state: &AppState) -> Option<McpResponse> {
    let id = req.id.clone();
    let is_notification = id.is_none();

    let result: Result<Value, McpError> = match McpMethod::parse(&req.method) {
        McpMethod::Initialize => Ok(initialize_result()),
        McpMethod::Initialized => return None,
        McpMethod::Ping => Ok(json!({})),
        McpMethod::ToolsList => Ok(json!({ "tools": tool_definitions() })),
        McpMethod::ToolsCall => tools_call(req.params, state).await,
        McpMethod::ResourcesList => resources_list().await,
        McpMethod::ResourcesRead => resources_read(req.params).await,
        McpMethod::Other(m) => {
            if is_notification {
                return None; // notification inconnue : silencieusement ignorée
            }
            Err(McpError { code: -32601, message: format!("méthode inconnue: {m}") })
        }
    };

    if is_notification {
        return None;
    }
    let id = id.unwrap_or(Value::Null);
    Some(match result {
        Ok(result) => McpResponse::ok(id, result),
        Err(error) => McpResponse::err(id, error),
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {}, "resources": {} },
        "serverInfo": { "name": "appmirror-mcp", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// Déclaration des outils exposés au LLM (JSON Schema des arguments).
fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_windows",
            "description": "Liste les fenêtres natives capturables (id + titre).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "attach_to_window",
            "description": "Cible une fenêtre pour l'injection d'entrées. Fournir window_id (obtenu via list_windows).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "window_id": { "type": "integer", "description": "Identifiant de fenêtre renvoyé par list_windows." },
                    "pid": { "type": "integer", "description": "PID (non supporté pour l'instant ; utiliser window_id)." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inject_input",
            "description": "Injecte une entrée souris/clavier dans la fenêtre attachée. Coordonnées normalisées [0,1] relatives à la fenêtre.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["mouse_move", "button", "click", "wheel", "key", "text"] },
                    "x": { "type": "number", "description": "Position X normalisée 0..1." },
                    "y": { "type": "number", "description": "Position Y normalisée 0..1." },
                    "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Bouton pour action=button/click." },
                    "pressed": { "type": "boolean", "description": "action=button/key : true=appui, false=relâche." },
                    "delta": { "type": "integer", "description": "Crans de molette (action=wheel), positif=haut." },
                    "code": { "type": "string", "description": "KeyboardEvent.code (action=key), ex: KeyA, Enter." },
                    "text": { "type": "string", "description": "Texte à saisir (action=text)." }
                },
                "required": ["action"],
                "additionalProperties": false
            }
        },
        {
            "name": "get_window_screenshot",
            "description": "Prend une capture d'écran de la fenêtre actuellement attachée.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "launch_app",
            "description": "Lance une application en tâche de fond sur la machine hôte.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "La commande ou le chemin de l'exécutable à lancer." },
                    "arguments": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments optionnels à passer à l'exécutable."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        }
    ])
}

async fn tools_call(params: Value, state: &AppState) -> Result<Value, McpError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError { code: -32602, message: "paramètre 'name' manquant".into() })?;
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let outcome: Result<Value, String> = match name {
        "list_windows" => tool_list_windows(state).map(|t| json!([ { "type": "text", "text": t } ])),
        "attach_to_window" => tool_attach(args, state).map(|t| json!([ { "type": "text", "text": t } ])),
        "inject_input" => tool_inject(args, state).map(|t| json!([ { "type": "text", "text": t } ])),
        "get_window_screenshot" => tool_screenshot(state).await.map(|b64| json!([ { "type": "image", "data": b64, "mimeType": "image/jpeg" } ])),
        "launch_app" => tool_launch(args).map(|t| json!([ { "type": "text", "text": t } ])),
        other => return Err(McpError { code: -32602, message: format!("outil inconnu: {other}") }),
    };

    Ok(match outcome {
        Ok(content) => json!({
            "content": content,
            "isError": false
        }),
        Err(err_text) => json!({
            "content": [ { "type": "text", "text": err_text } ],
            "isError": true
        }),
    })
}

fn tool_list_windows(state: &AppState) -> Result<String, String> {
    let locked_apps = {
        let cfg_opt = state.config.lock().unwrap();
        if let Some(cfg_mutex) = &*cfg_opt {
            cfg_mutex.lock().unwrap().locked_apps.clone()
        } else {
            Vec::new()
        }
    };
    let items = capture::list_windows(&locked_apps).map_err(|e| format!("échec list_windows: {e}"))?;
    let windows: Vec<Value> = items
        .iter()
        .map(|w| json!({ "id": w.id, "title": w.title, "locked": w.locked }))
        .collect();
    Ok(serde_json::to_string_pretty(&json!({ "windows": windows })).unwrap_or_default())
}

fn tool_attach(args: Value, state: &AppState) -> Result<String, String> {
    let parsed: AttachArgs =
        serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let id = match (parsed.window_id, parsed.pid) {
        (Some(id), _) => id,
        (None, Some(_pid)) => {
            return Err("attache par PID non supportée pour l'instant ; fournir window_id".into())
        }
        (None, None) => return Err("fournir window_id (voir list_windows)".into()),
    };

    // Vérifie l'existence de la fenêtre et récupère sa taille client (repère de
    // normalisation), puis la met au premier plan (cible de SendInput/XTest).
    let (width, height) =
        input::client_size(id).ok_or_else(|| format!("fenêtre {id} introuvable"))?;
    input::focus_window(id);
    *state.attached.lock().unwrap() = Some(AttachedWindow { id, width, height });

    // Informe le superviseur humain si un WebSocket est connecté (texte affiché
    // dans l'UI, donc en anglais).
    let _ = state.notices.send(format!("MCP agent attached to window {id} ({width}×{height})"));
    Ok(format!("attached to window {id} ({width}×{height})"))
}

/// Dénormalise (x,y) ∈ [0,1] vers des pixels client de la fenêtre attachée.
fn denorm(target: &AttachedWindow, x: Option<f64>, y: Option<f64>) -> (i32, i32) {
    let nx = (x.unwrap_or(0.0).clamp(0.0, 1.0) * (target.width.max(1) - 1) as f64).round() as i32;
    let ny = (y.unwrap_or(0.0).clamp(0.0, 1.0) * (target.height.max(1) - 1) as f64).round() as i32;
    (nx, ny)
}

fn tool_inject(args: Value, state: &AppState) -> Result<String, String> {
    let inj: InjectArgs =
        serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let target = *state.attached.lock().unwrap();
    let Some(target) = target else {
        return Err("aucune fenêtre attachée (appeler attach_to_window d'abord)".into());
    };

    let report = match inj.action {
        InjectAction::MouseMove => {
            let (x, y) = denorm(&target, inj.x, inj.y);
            input::mouse_move(target.id, x, y);
            format!("mouse_move -> ({x},{y})")
        }
        InjectAction::Button => {
            let (x, y) = denorm(&target, inj.x, inj.y);
            let button = inj.button.unwrap_or(MouseButton::Left);
            let pressed = inj.pressed.unwrap_or(true);
            input::mouse_button(target.id, button, pressed, x, y);
            format!("button {button:?} pressed={pressed} @({x},{y})")
        }
        InjectAction::Click => {
            let (x, y) = denorm(&target, inj.x, inj.y);
            let button = inj.button.unwrap_or(MouseButton::Left);
            input::mouse_button(target.id, button, true, x, y);
            input::mouse_button(target.id, button, false, x, y);
            format!("click {button:?} @({x},{y})")
        }
        InjectAction::Wheel => {
            let (x, y) = denorm(&target, inj.x, inj.y);
            let delta = inj.delta.unwrap_or(1);
            input::wheel(target.id, delta, x, y);
            format!("wheel {delta} @({x},{y})")
        }
        InjectAction::Key => {
            let code = inj.code.ok_or_else(|| "action=key requiert 'code'".to_string())?;
            let pressed = inj.pressed.unwrap_or(true);
            input::key(&code, pressed);
            format!("key {code} pressed={pressed}")
        }
        InjectAction::Text => {
            let text = inj.text.ok_or_else(|| "action=text requiert 'text'".to_string())?;
            let n = type_text(&text);
            format!("texte saisi ({n} caractères)")
        }
    };
    Ok(report)
}

/// Saisie best-effort d'une chaîne : décompose chaque caractère en appui/relâche
/// via la façade `input::key` (gère la touche Shift multiplateforme).
fn type_text(text: &str) -> usize {
    let mut count = 0;
    for ch in text.chars() {
        let Some((code, shift)) = char_to_code(ch) else {
            tracing::debug!("caractère non saisissable ignoré: {ch:?}");
            continue;
        };
        if shift {
            input::key("ShiftLeft", true);
        }
        input::key(&code, true);
        input::key(&code, false);
        if shift {
            input::key("ShiftLeft", false);
        }
        count += 1;
    }
    count
}

/// Mappe un caractère ASCII usuel vers (`KeyboardEvent.code`, shift requis).
fn char_to_code(ch: char) -> Option<(String, bool)> {
    match ch {
        'a'..='z' => Some((format!("Key{}", ch.to_ascii_uppercase()), false)),
        'A'..='Z' => Some((format!("Key{ch}"), true)),
        '0'..='9' => Some((format!("Digit{ch}"), false)),
        ' ' => Some(("Space".into(), false)),
        '\n' => Some(("Enter".into(), false)),
        '\t' => Some(("Tab".into(), false)),
        _ => None,
    }
}

// ---- Ressources : les captures disque (voir `recorder.rs`) ----

/// URI d'accès aux captures. Les fichiers restent sur l'hôte du backend.
const CAPTURE_URI_PREFIX: &str = "appmirror://capture/";

async fn resources_list() -> Result<Value, McpError> {
    let base = recorder::default_dir();
    let mut resources = Vec::new();

    let Ok(mut entries) = tokio::fs::read_dir(&base).await else {
        // Dossier de captures absent => liste vide (pas une erreur).
        return Ok(json!({ "resources": resources }));
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.is_dir() {
            // Un niveau de sous-dossier (les séquences `rec_<horodatage>/`).
            if let Ok(mut sub) = tokio::fs::read_dir(&path).await {
                while let Ok(Some(f)) = sub.next_entry().await {
                    push_if_jpeg(&base, &f.path(), &mut resources);
                }
            }
        } else {
            push_if_jpeg(&base, &path, &mut resources);
        }
    }
    Ok(json!({ "resources": resources }))
}

fn push_if_jpeg(base: &Path, path: &Path, out: &mut Vec<Value>) {
    let is_jpeg = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
        .unwrap_or(false);
    if !is_jpeg {
        return;
    }
    let Ok(rel) = path.strip_prefix(base) else { return };
    let rel = rel.to_string_lossy().replace('\\', "/");
    out.push(json!({
        "uri": format!("{CAPTURE_URI_PREFIX}{rel}"),
        "name": path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "mimeType": "image/jpeg"
    }));
}

async fn resources_read(params: Value) -> Result<Value, McpError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError { code: -32602, message: "paramètre 'uri' manquant".into() })?;
    let rel = uri
        .strip_prefix(CAPTURE_URI_PREFIX)
        .ok_or_else(|| McpError { code: -32602, message: format!("uri non supportée: {uri}") })?;

    let base = recorder::default_dir();
    let canon_base = base
        .canonicalize()
        .map_err(|e| McpError { code: -32603, message: format!("dossier captures indisponible: {e}") })?;
    // Anti-traversal : le chemin résolu doit rester sous le dossier de captures.
    let canon = base
        .join(rel)
        .canonicalize()
        .map_err(|_| McpError { code: -32602, message: "ressource introuvable".into() })?;
    if !canon.starts_with(&canon_base) {
        return Err(McpError { code: -32602, message: "chemin hors du dossier de captures".into() });
    }

    let bytes = tokio::fs::read(&canon)
        .await
        .map_err(|e| McpError { code: -32603, message: format!("lecture échouée: {e}") })?;
    let blob = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(json!({
        "contents": [ { "uri": uri, "mimeType": "image/jpeg", "blob": blob } ]
    }))
}

async fn tool_screenshot(state: &AppState) -> Result<String, String> {
    let target = *state.attached.lock().unwrap();
    let Some(target) = target else {
        return Err("aucune fenêtre attachée (appeler attach_to_window d'abord)".into());
    };

    // Démarre une session de capture temporaire
    // Qualité: 80, Échelle: 1.0, Curseur: vrai
    let (session, mut rx) = capture::start(target.id, 80, 1.0, true)
        .map_err(|e| format!("impossible de démarrer la capture: {e}"))?;

    // Attendre la première frame non vide
    let mut frame_bytes = rx.borrow().clone();
    if frame_bytes.is_empty() {
        tokio::select! {
            res = rx.changed() => {
                if res.is_ok() {
                    frame_bytes = rx.borrow().clone();
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(1500)) => {}
        }
    }

    // Arrêter la capture immédiatement
    let _ = session.stop();

    if frame_bytes.is_empty() {
        return Err("délai dépassé pour recevoir la frame de capture".into());
    }

    // Encoder en base64
    let b64 = base64::engine::general_purpose::STANDARD.encode(&frame_bytes);
    Ok(b64)
}

fn tool_launch(args: Value) -> Result<String, String> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'command' manquant".to_string())?;

    let arguments: Vec<String> = if let Some(arr) = args.get("arguments").and_then(Value::as_array) {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    } else {
        Vec::new()
    };

    // Lance le processus en tâche de fond
    let mut cmd = std::process::Command::new(command);
    cmd.args(&arguments);

    // Sous Windows, empêcher l'apparition de la console noire pour les apps GUI
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    match cmd.spawn() {
        Ok(child) => {
            let pid = child.id();
            Ok(format!("application lancée avec succès (PID: {pid})"))
        }
        Err(e) => Err(format!("impossible de lancer l'application: {e}")),
    }
}

