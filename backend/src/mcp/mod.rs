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
    AttachArgs, CancelExecutionArgs, ExecuteWorkflowArgs, GetAppProfileArgs, GetExecutionStatusArgs,
    GetLayoutArgs, InjectAction, InjectArgs, LayoutFile, LayoutZoneKind, ListLayoutsArgs, McpError,
    McpMethod, McpRequest, McpResponse, MouseButton, PlayMacroArgs, SaveMacroArgs, SaveWorkflowArgs,
    SetAppProfileArgs,
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
                    "action": { "type": "string", "enum": ["mouse_move", "button", "click", "double_click", "drag", "wheel", "key", "text"] },
                    "x": { "type": "number", "description": "Position X normalisée 0..1 (origine du drag)." },
                    "y": { "type": "number", "description": "Position Y normalisée 0..1 (origine du drag)." },
                    "to_x": { "type": "number", "description": "Destination X normalisée 0..1 (action=drag)." },
                    "to_y": { "type": "number", "description": "Destination Y normalisée 0..1 (action=drag)." },
                    "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Bouton pour action=button/click/double_click/drag." },
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
            "name": "focus_window",
            "description": "Restaure (si minimisée) et met au premier plan la fenêtre attachée. À utiliser si une capture échoue ou si le focus a été perdu.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
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
        },
        {
            "name": "inspect_ui_elements",
            "description": "Examine la fenêtre attachée et renvoie à la fois les contrôles natifs détectés et les layouts UI définis par l'utilisateur pour son application.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }
        ,{
            "name": "list_layouts",
            "description": "Liste les plans UI disponibles. Sans contexte, utilise l'application de la fenêtre MCP attachée.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string", "description": "Nom de processus/contexte optionnel." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "get_layout",
            "description": "Renvoie un ou plusieurs plans UI utilisateur avec les types de zones et leurs coordonnées normalisées [0,1]. Utilisez le centre de bounds pour injecter un clic ou cibler une saisie.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string", "description": "Contexte applicatif; déduit de la fenêtre attachée si absent." },
                    "name": { "type": "string", "description": "Nom du layout; tous les layouts du contexte si absent." }
                },
                "additionalProperties": false
            }
        }
        ,{
            "name": "get_application_state",
            "description": "Détecte l'état applicatif courant (main_menu, gameplay…) en comparant la frame aux signatures d'états enregistrées pour le contexte. Renvoie l'état détecté avec sa confiance et les autres candidats.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string", "description": "Contexte applicatif; déduit de la fenêtre attachée si absent." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "detect_layout",
            "description": "Détecte l'état courant puis renvoie le layout UI associé (zones + coordonnées). À utiliser avant activate_ui_element quand l'interface a plusieurs variantes (ex. menu avec/sans Continuer).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string", "description": "Contexte applicatif; déduit de la fenêtre attachée si absent." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "wait_for_ui_state",
            "description": "Attend que l'application atteigne un état enregistré (sondage ~4 Hz). Échoue avec le dernier candidat détecté si le délai expire.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "state": { "type": "string", "description": "Nom de l'état attendu." },
                    "timeout_ms": { "type": "integer", "description": "Délai maximal en ms (défaut 5000, max 60000)." },
                    "context": { "type": "string", "description": "Contexte applicatif; déduit de la fenêtre attachée si absent." }
                },
                "required": ["state"],
                "additionalProperties": false
            }
        }
        ,{
            "name": "activate_ui_element",
            "description": "Active un élément d'un layout UI (bouton, entrée de menu…) avec stratégies de repli et vérification visuelle. Résout l'élément par id, nom ou label de zone; essaie clic, puis clic+Entrée, puis double-clic (strategy=auto) jusqu'à observer un changement d'écran. Rapport détaillé de chaque tentative.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "element": { "type": "string", "description": "Id, nom ou label de la zone à activer." },
                    "layout": { "type": "string", "description": "Nom du layout; sinon recherche dans tous les layouts du contexte." },
                    "context": { "type": "string", "description": "Contexte applicatif; déduit de la fenêtre attachée si absent." },
                    "strategy": { "type": "string", "enum": ["auto", "click", "click_then_enter", "double_click"], "description": "auto (défaut) enchaîne les replis; sinon force une stratégie unique." },
                    "verify": { "type": "boolean", "description": "Vérifier le changement d'écran après activation (défaut true)." }
                },
                "required": ["element"],
                "additionalProperties": false
            }
        },
        {
            "name": "type_into_ui_element",
            "description": "Clique sur une zone de layout (champ de saisie…) puis y tape le texte donné. Résout l'élément par id, nom ou label.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "element": { "type": "string", "description": "Id, nom ou label de la zone cible." },
                    "text": { "type": "string", "description": "Texte à saisir après le clic." },
                    "layout": { "type": "string", "description": "Nom du layout; sinon recherche dans tous les layouts du contexte." },
                    "context": { "type": "string", "description": "Contexte applicatif; déduit de la fenêtre attachée si absent." }
                },
                "required": ["element", "text"],
                "additionalProperties": false
            }
        }
        ,{
            "name": "describe_context",
            "description": "Inventaire complet de ce que le serveur connaît d'une application : layouts UI, états visuels, macros et workflows enregistrés. À appeler en premier quand on découvre une interface — tout ce qui est listé est réutilisable, et tout ce que vous créez (save_layout, capture_ui_state, save_macro, save_workflow) devient visible ici et éditable par l'utilisateur dans son interface.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string", "description": "Contexte applicatif (nom de processus); déduit de la fenêtre attachée si absent." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "save_layout",
            "description": "Crée ou remplace un layout UI pour un contexte. Les zones (coordonnées normalisées [0,1]) peuvent porter label, description, séquence d'activation, expectedState et ancre relative. Le layout devient utilisable par activate_ui_element et éditable par l'utilisateur dans le panneau Layout.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Nom du layout (ex. main_menu_with_continue)." },
                    "context": { "type": "string", "description": "Contexte applicatif; déduit de la fenêtre attachée si absent." },
                    "source_width": { "type": "integer", "description": "Largeur client de référence; taille de la fenêtre attachée si absent." },
                    "source_height": { "type": "integer", "description": "Hauteur client de référence; taille de la fenêtre attachée si absent." },
                    "zones": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "Zones au format LayoutZone: { id, name, kind, bounds:{x,y,width,height}, label?, description?, activation?, expectedState?, anchor? }."
                    }
                },
                "required": ["name", "zones"],
                "additionalProperties": false
            }
        },
        {
            "name": "capture_ui_state",
            "description": "Enregistre l'écran actuel de la fenêtre attachée comme état nommé et reconnaissable : chaque zone du layout indiqué est hashée sur la frame courante. Ensuite get_application_state / wait_for_ui_state / detect_layout sauront reconnaître cet écran. Amenez d'abord l'application sur l'écran voulu.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Nom de l'état (ex. main_menu, gameplay, options)." },
                    "layout": { "type": "string", "description": "Layout dont les zones servent de signatures; requis si 'zones' absent." },
                    "zones": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "Zones explicites à hasher (format LayoutZone); sinon celles du layout."
                    },
                    "context": { "type": "string", "description": "Contexte applicatif; déduit de la fenêtre attachée si absent." }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_macros",
            "description": "Liste les macros enregistrées (contexte, nom, nombre d'étapes), incluant celles créées par l'utilisateur ou un autre agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string", "description": "Filtre par contexte applicatif; toutes si absent." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "get_macro",
            "description": "Renvoie la définition complète d'une macro (étapes JSON) pour l'inspecter ou la modifier puis la re-sauvegarder via save_macro.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Nom de la macro." },
                    "context": { "type": "string", "description": "Contexte, si plusieurs macros portent le même nom." }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        },
        {
            "name": "get_workflow",
            "description": "Renvoie le graphe complet d'un workflow (nœuds + arêtes) pour l'inspecter ou le modifier puis le re-sauvegarder via save_workflow.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Nom du workflow." }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }
        ,{
            "name": "save_macro",
            "description": "Sauvegarde une macro liée à un contexte de processus. Étapes brutes (mouse_move, mouse_button, wheel, key, text) et sémantiques (activate_element, type_into_element, wait_for_state, wait_ms, call_macro). Les champs texte acceptent {{param}} substitué à l'exécution (execute_macro).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string" },
                    "name": { "type": "string" },
                    "window_id": { "type": "integer" },
                    "params": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "Paramètres déclarés: { name, kind: text|number|boolean|secret|choice|zone_ref, default?, description?, choices? }."
                    },
                    "steps": { "type": "array", "items": { "type": "object" } }
                },
                "required": ["context", "name", "window_id", "steps"],
                "additionalProperties": false
            }
        },
        {
            "name": "execute_macro",
            "description": "Exécute une macro sur la fenêtre attachée avec des paramètres nommés (substitués dans les étapes via {{nom}}). Supporte les étapes sémantiques (activate_element, type_into_element, wait_for_state, call_macro) qui référencent les layouts/états du contexte. Préférer à play_macro pour les macros paramétrées.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Nom de la macro." },
                    "context": { "type": "string", "description": "Contexte, si plusieurs macros portent le même nom." },
                    "parameters": { "type": "object", "description": "Valeurs des paramètres déclarés, ex. {\"username\": \"manu\"}." },
                    "injection_mode": { "type": "string", "enum": ["foreground", "background"], "description": "background (défaut) n'affecte pas le focus; foreground requis pour les apps WinUI/DirectX. Les étapes sémantiques gèrent leur focus elles-mêmes." }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        },
        {
            "name": "play_macro",
            "description": "Rejoue une macro sur la fenêtre MCP attachée, en injection arrière-plan sans voler le focus.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
                "additionalProperties": false
            }
        }
        ,{
            "name": "save_workflow",
            "description": "Valide et sauvegarde un graphe de workflow visuel.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "graph": {
                        "type": "object",
                        "properties": {
                            "nodes": { "type": "object" },
                            "edges": { "type": "array", "items": { "type": "object" } }
                        },
                        "required": ["nodes", "edges"],
                        "additionalProperties": false
                    }
                },
                "required": ["name", "graph"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_workflows",
            "description": "Liste les graphes de workflow sauvegardés.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "execute_workflow",
            "description": "Exécute un workflow sauvegardé sur la fenêtre MCP attachée. Passer observe=true pour lancer en tâche de fond et obtenir un execution_id immédiatement.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "observe": { "type": "boolean", "description": "Lancer de façon asynchrone et obtenir l'execution_id pour suivi." }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        },
        {
            "name": "get_execution_status",
            "description": "Rapporte le statut (running/finished/failed/cancelled), l'historique par nœud et les durées d'une exécution de workflow.",
            "inputSchema": {
                "type": "object",
                "properties": { "execution_id": { "type": "string" } },
                "required": ["execution_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "cancel_execution",
            "description": "Demande l'annulation d'une exécution de workflow en cours.",
            "inputSchema": {
                "type": "object",
                "properties": { "execution_id": { "type": "string" } },
                "required": ["execution_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "set_app_profile",
            "description": "Définit ou met à jour le profil de capacités d'injection d'une application (ex: foreground_required, preferred_activation).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string" },
                    "foreground_required": { "type": "boolean" },
                    "preferred_activation": { "type": "string", "enum": ["auto", "click", "keyboard"] },
                    "background_mouse": { "type": "string", "enum": ["supported", "partial", "unsupported"] },
                    "background_keyboard": { "type": "string", "enum": ["supported", "partial", "unsupported"] },
                    "pause_on_focus_loss": { "type": "boolean" }
                },
                "required": ["context"],
                "additionalProperties": false
            }
        },
        {
            "name": "get_app_profile",
            "description": "Rapporte le profil de capacités d'injection enregistré pour une application.",
            "inputSchema": {
                "type": "object",
                "properties": { "context": { "type": "string" } },
                "required": ["context"],
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
        "focus_window" => tool_focus(state).map(|t| json!([ { "type": "text", "text": t } ])),
        "inject_input" => tool_inject(args, state).map(|t| json!([ { "type": "text", "text": t } ])),
        "get_window_screenshot" => tool_screenshot(state).await.map(|b64| json!([ { "type": "image", "data": b64, "mimeType": "image/jpeg" } ])),
        "launch_app" => tool_launch(args, state).map(|t| json!([ { "type": "text", "text": t } ])),
        "inspect_ui_elements" => tool_inspect(state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "get_application_state" => tool_get_state(args, state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "detect_layout" => tool_detect_layout(args, state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "wait_for_ui_state" => tool_wait_state(args, state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "activate_ui_element" => tool_activate(args, state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "type_into_ui_element" => tool_type_into(args, state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "list_layouts" => tool_list_layouts(args, state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "get_layout" => tool_get_layout(args, state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "describe_context" => tool_describe_context(args, state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "save_layout" => tool_save_layout(args, state).await.map(|t| json!([ { "type": "text", "text": t } ])),
        "capture_ui_state" => tool_capture_state(args, state).await.map(|t| json!([ { "type": "text", "text": t } ])),
        "list_macros" => tool_list_macros(args).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "get_macro" => tool_get_macro(args).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "get_workflow" => tool_get_workflow(args).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "save_macro" => tool_save_macro(args).await.map(|t| json!([ { "type": "text", "text": t } ])),
        "play_macro" => tool_play_macro(args, state).await.map(|t| json!([ { "type": "text", "text": t } ])),
        "execute_macro" => tool_execute_macro(args, state).await.map(|t| json!([ { "type": "text", "text": t } ])),
        "save_workflow" => tool_save_workflow(args).await.map(|t| json!([ { "type": "text", "text": t } ])),
        "list_workflows" => tool_list_workflows().await.map(|t| json!([ { "type": "text", "text": t } ])),
        "execute_workflow" => tool_execute_workflow(args, state).await.map(|t| json!([ { "type": "text", "text": t } ])),
        "get_execution_status" => tool_get_execution_status(args, state).map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
        "cancel_execution" => tool_cancel_execution(args, state).map(|t| json!([ { "type": "text", "text": t } ])),
        "set_app_profile" => tool_set_app_profile(args).await.map(|t| json!([ { "type": "text", "text": t } ])),
        "get_app_profile" => tool_get_app_profile(args, state).await.map(|v| json!([ { "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() } ])),
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

async fn tool_save_workflow(args: Value) -> Result<String, String> {
    let parsed: SaveWorkflowArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let path = crate::workflow_store::save(&parsed.name, parsed.graph).await?;
    Ok(format!("workflow sauvegardé: {}", path.display()))
}

async fn tool_list_workflows() -> Result<String, String> {
    let items = crate::workflow_store::list().await?;
    serde_json::to_string_pretty(&items).map_err(|e| format!("sérialisation: {e}"))
}

async fn tool_execute_workflow(args: Value, state: &AppState) -> Result<String, String> {
    let parsed: ExecuteWorkflowArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let target = *state.attached.lock().unwrap();
    let file = crate::workflow_store::load(&parsed.name).await?;
    let execution_id = uuid::Uuid::new_v4().to_string();

    if parsed.observe {
        let state_clone = state.clone();
        let name = parsed.name.clone();
        let exec_id = execution_id.clone();
        tokio::spawn(async move {
            let _ = crate::workflow::run_full(file, target, None, Some(state_clone), None, Some(exec_id)).await;
        });
        let _ = state.notices.send(format!("MCP launched workflow '{name}' asynchronously ({execution_id})"));
        Ok(json!({
            "executionId": execution_id,
            "status": "running",
            "workflowName": parsed.name,
        }).to_string())
    } else {
        let count = crate::workflow::run_full(file, target, None, Some(state.clone()), None, Some(execution_id)).await?;
        let _ = state.notices.send(format!("MCP executed workflow '{}' ({count} nodes)", parsed.name));
        Ok(format!("workflow '{}' exécuté: {count} nœuds", parsed.name))
    }
}

fn tool_get_execution_status(args: Value, state: &AppState) -> Result<Value, String> {
    let parsed: GetExecutionStatusArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let record = state.get_execution(&parsed.execution_id).ok_or_else(|| format!("exécution '{}' introuvable", parsed.execution_id))?;
    Ok(serde_json::to_value(record).unwrap_or_default())
}

fn tool_cancel_execution(args: Value, state: &AppState) -> Result<String, String> {
    let parsed: CancelExecutionArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    if state.cancel_execution(&parsed.execution_id) {
        Ok(format!("annulation demandée pour l'exécution '{}'", parsed.execution_id))
    } else {
        Err(format!("exécution '{}' introuvable", parsed.execution_id))
    }
}

async fn tool_set_app_profile(args: Value) -> Result<String, String> {
    let parsed: SetAppProfileArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let mut profile = crate::profile_store::load_or_default(&parsed.context).await;
    if let Some(v) = parsed.foreground_required { profile.foreground_required = v; }
    if let Some(v) = parsed.preferred_activation { profile.preferred_activation = v; }
    if let Some(v) = parsed.background_mouse { profile.background_mouse = v; }
    if let Some(v) = parsed.background_keyboard { profile.background_keyboard = v; }
    if let Some(v) = parsed.pause_on_focus_loss { profile.pause_on_focus_loss = v; }
    let path = crate::profile_store::save(&profile).await?;
    Ok(format!("profil d'application pour '{}' sauvegardé ({})", profile.context, path.display()))
}

async fn tool_get_app_profile(args: Value, _state: &AppState) -> Result<Value, String> {
    let parsed: GetAppProfileArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let profile = crate::profile_store::load_or_default(&parsed.context).await;
    Ok(serde_json::to_value(profile).unwrap_or_default())
}

/// Contexte explicite ou déduit de la fenêtre attachée.
fn context_or_attached(args: &Value, state: &AppState) -> Result<String, String> {
    match args.get("context").and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => Ok(value.trim().to_lowercase()),
        _ => attached_context(state),
    }
}

async fn tool_describe_context(args: Value, state: &AppState) -> Result<Value, String> {
    let context = context_or_attached(&args, state)?;
    let profile = crate::profile_store::load_or_default(&context).await;
    let layouts = crate::layout_store::list(Some(&context)).await.unwrap_or_default();
    let states = crate::state_store::list(Some(&context)).await.unwrap_or_default();
    let macros: Vec<_> = crate::macro_store::list()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.context.eq_ignore_ascii_case(&context))
        .collect();
    // Les workflows ne sont pas indexés par contexte (leur cible vit dans le
    // nœud START) : la liste est globale.
    let workflows = crate::workflow_store::list().await.unwrap_or_default();
    Ok(json!({
        "context": context,
        "profile": profile,
        "layouts": layouts,
        "states": states,
        "macros": macros,
        "workflows": workflows,
        "workflowsNote": "liste globale; la cible d'un workflow est définie par son nœud START (voir get_workflow)",
        "authoring": {
            "profile": "set_app_profile configure les capacités d'injection (ex: foreground_required, preferred_activation)",
            "layouts": "save_layout crée/remplace un layout; l'utilisateur peut ensuite l'éditer dans le panneau Layout",
            "states": "capture_ui_state hashe l'écran courant pour le rendre reconnaissable (get_application_state, wait_for_ui_state, detect_layout)",
            "macros": "save_macro persiste une séquence d'étapes; get_macro pour relire/modifier",
            "workflows": "save_workflow persiste un graphe; get_workflow pour relire/modifier"
        },
    }))
}

async fn tool_save_layout(args: Value, state: &AppState) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'name' manquant".to_string())?
        .to_string();
    let context = context_or_attached(&args, state)?;
    let zones: Vec<crate::protocol::LayoutZone> = serde_json::from_value(
        args.get("zones").cloned().ok_or_else(|| "paramètre 'zones' manquant".to_string())?,
    )
    .map_err(|e| format!("zones invalides: {e}"))?;
    let (default_w, default_h) = {
        let target = *state.attached.lock().unwrap();
        target
            .and_then(|t| input::client_size(t.id))
            .map(|(w, h)| (w.max(1) as u32, h.max(1) as u32))
            .unwrap_or((1280, 720))
    };
    let source_width = args.get("source_width").and_then(Value::as_u64).map(|v| v as u32).unwrap_or(default_w);
    let source_height = args.get("source_height").and_then(Value::as_u64).map(|v| v as u32).unwrap_or(default_h);
    let path = crate::layout_store::save(crate::protocol::SaveLayoutArgs {
        context: context.clone(),
        name: name.clone(),
        source_width,
        source_height,
        zones,
    })
    .await?;
    let _ = state.notices.send(format!("MCP saved layout '{name}' for {context}"));
    Ok(format!("layout '{name}' sauvegardé pour {context}: {}", path.display()))
}

async fn tool_capture_state(args: Value, state: &AppState) -> Result<String, String> {
    let target = *state.attached.lock().unwrap();
    let Some(target) = target else {
        return Err("aucune fenêtre attachée (appeler attach_to_window d'abord)".into());
    };
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'name' manquant".to_string())?
        .to_string();
    let context = context_or_attached(&args, state)?;
    let layout_name = args.get("layout").and_then(Value::as_str).map(str::to_string);
    let zones: Vec<crate::protocol::LayoutZone> = match args.get("zones") {
        Some(value) if !value.is_null() => {
            serde_json::from_value(value.clone()).map_err(|e| format!("zones invalides: {e}"))?
        }
        _ => {
            let layout = layout_name
                .as_deref()
                .ok_or_else(|| "fournir 'layout' (zones d'un layout sauvegardé) ou 'zones' explicites".to_string())?;
            crate::layout_store::load_named(&context, layout).await?.zones
        }
    };
    if zones.is_empty() {
        return Err("aucune zone à hasher".into());
    }
    let frame = capture_frame(target.id, false).await?;
    let img = image::load_from_memory(&frame).map_err(|e| format!("décodage frame: {e}"))?;
    let signatures: Vec<crate::protocol::StateSignature> = zones
        .iter()
        .map(|zone| {
            let (dhash, mean_luma) = crate::detect::zone_signature(&img, &zone.bounds);
            crate::protocol::StateSignature {
                zone_id: zone.id.clone(),
                bounds: zone.bounds,
                dhash: format!("{dhash:016x}"),
                mean_luma,
            }
        })
        .collect();
    let count = signatures.len();
    let path = crate::state_store::save(crate::protocol::UiStateFile {
        version: 1,
        context: context.clone(),
        name: name.clone(),
        layout: layout_name,
        signatures,
    })
    .await?;
    let _ = state.notices.send(format!("MCP captured state '{name}' for {context}"));
    Ok(format!("état '{name}' capturé pour {context} ({count} signatures): {}", path.display()))
}

async fn tool_list_macros(args: Value) -> Result<Value, String> {
    let context = args.get("context").and_then(Value::as_str).map(|v| v.trim().to_lowercase());
    let items: Vec<_> = crate::macro_store::list()
        .await?
        .into_iter()
        .filter(|m| context.as_deref().is_none_or(|wanted| m.context.eq_ignore_ascii_case(wanted)))
        .collect();
    serde_json::to_value(items).map_err(|e| format!("sérialisation macros: {e}"))
}

async fn tool_get_macro(args: Value) -> Result<Value, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'name' manquant".to_string())?;
    let file = match args.get("context").and_then(Value::as_str) {
        Some(context) if !context.trim().is_empty() => crate::macro_store::load_named(context, name).await?,
        _ => crate::macro_store::load_by_name(name).await?,
    };
    serde_json::to_value(file).map_err(|e| format!("sérialisation macro: {e}"))
}

async fn tool_get_workflow(args: Value) -> Result<Value, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'name' manquant".to_string())?;
    let file = crate::workflow_store::load(name).await?;
    serde_json::to_value(file).map_err(|e| format!("sérialisation workflow: {e}"))
}

async fn tool_execute_macro(args: Value, state: &AppState) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'name' manquant".to_string())?;
    let target = state.attached.lock().unwrap().ok_or_else(|| "aucune fenêtre attachée (appeler attach_to_window d'abord)".to_string())?;
    let file = match args.get("context").and_then(Value::as_str) {
        Some(context) if !context.trim().is_empty() => crate::macro_store::load_named(context, name).await?,
        _ => crate::macro_store::load_by_name(name).await?,
    };
    let actual_context = attached_context(state)?;
    if actual_context != file.context.to_lowercase() {
        return Err(format!("contexte incompatible: macro={}, cible={actual_context}", file.context));
    }
    // Valeurs de paramètres : chaînes, nombres et booléens acceptés.
    let mut parameters = std::collections::HashMap::new();
    if let Some(object) = args.get("parameters").and_then(Value::as_object) {
        for (key, value) in object {
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                other => return Err(format!("paramètre '{key}': type non supporté ({other})")),
            };
            parameters.insert(key.clone(), text);
        }
    }
    let mode = match args.get("injection_mode").and_then(Value::as_str) {
        Some("foreground") => crate::protocol::InjectionMode::Foreground,
        _ => crate::protocol::InjectionMode::Background,
    };
    let count = crate::macro_player::play_full(&file, target, mode, &parameters, 0).await?;
    let _ = state.notices.send(format!("MCP executed macro '{}' ({count} steps)", file.name));
    Ok(format!("macro '{}' exécutée: {count} étapes", file.name))
}

async fn tool_save_macro(args: Value) -> Result<String, String> {
    let parsed: SaveMacroArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let path = crate::macro_store::save(parsed).await?;
    Ok(format!("macro sauvegardée: {}", path.display()))
}

async fn tool_play_macro(args: Value, state: &AppState) -> Result<String, String> {
    let parsed: PlayMacroArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let target = state.attached.lock().unwrap().ok_or_else(|| "aucune fenêtre attachée (appeler attach_to_window d'abord)".to_string())?;
    let file = crate::macro_store::load_by_name(&parsed.name).await?;
    let (locked_apps, allow_virtual) = {
        let cfg_opt = state.config.lock().unwrap();
        if let Some(cfg_mutex) = &*cfg_opt {
            let cfg = cfg_mutex.lock().unwrap();
            (cfg.locked_apps.clone(), cfg.allow_virtual_desktop)
        } else { (Vec::new(), false) }
    };
    let windows = capture::list_windows(&locked_apps, allow_virtual).map_err(|e| format!("vérification du contexte: {e}"))?;
    let actual_context = windows.iter().find(|w| w.id == target.id).map(|w| w.context.to_lowercase())
        .ok_or_else(|| format!("fenêtre cible {} introuvable", target.id))?;
    if actual_context != file.context.to_lowercase() {
        return Err(format!("contexte incompatible: macro={}, cible={actual_context}", file.context));
    }
    let count = crate::macro_player::play(&file, target).await?;
    let _ = state.notices.send(format!("MCP played macro '{}' ({count} steps)", file.name));
    Ok(format!("macro '{}' exécutée: {count} étapes", file.name))
}

fn tool_list_windows(state: &AppState) -> Result<String, String> {
    let (locked_apps, allow_virtual) = {
        let cfg_opt = state.config.lock().unwrap();
        if let Some(cfg_mutex) = &*cfg_opt {
            let cfg = cfg_mutex.lock().unwrap();
            (cfg.locked_apps.clone(), cfg.allow_virtual_desktop)
        } else {
            (Vec::new(), false)
        }
    };
    let items = capture::list_windows(&locked_apps, allow_virtual).map_err(|e| format!("échec list_windows: {e}"))?;
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

/// Restaure et met au premier plan la fenêtre attachée (récupération de focus).
fn tool_focus(state: &AppState) -> Result<String, String> {
    let target = *state.attached.lock().unwrap();
    let Some(target) = target else {
        return Err("aucune fenêtre attachée (appeler attach_to_window d'abord)".into());
    };
    input::focus_window(target.id);
    Ok(format!("fenêtre {} restaurée et mise au premier plan", target.id))
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
        InjectAction::DoubleClick => {
            let (x, y) = denorm(&target, inj.x, inj.y);
            let button = inj.button.unwrap_or(MouseButton::Left);
            for _ in 0..2 {
                input::mouse_button(target.id, button, true, x, y);
                input::mouse_button(target.id, button, false, x, y);
            }
            format!("double_click {button:?} @({x},{y})")
        }
        InjectAction::Drag => {
            let (x1, y1) = denorm(&target, inj.x, inj.y);
            let (x2, y2) = denorm(&target, inj.to_x, inj.to_y);
            let button = inj.button.unwrap_or(MouseButton::Left);
            // Vrai geste : appui, déplacements interpolés (avec un court délai pour
            // que la cible enregistre le drag), puis relâche.
            input::mouse_move(target.id, x1, y1);
            input::mouse_button(target.id, button, true, x1, y1);
            const STEPS: i32 = 12;
            for i in 1..=STEPS {
                let t = i as f64 / STEPS as f64;
                let ix = (x1 as f64 + (x2 - x1) as f64 * t).round() as i32;
                let iy = (y1 as f64 + (y2 - y1) as f64 * t).round() as i32;
                input::mouse_move(target.id, ix, iy);
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
            input::mouse_button(target.id, button, false, x2, y2);
            format!("drag {button:?} ({x1},{y1}) -> ({x2},{y2})")
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
            input::key(Some(target.id), &code, pressed);
            format!("key {code} pressed={pressed}")
        }
        InjectAction::Text => {
            let text = inj.text.ok_or_else(|| "action=text requiert 'text'".to_string())?;
            let n = type_text(Some(target.id), &text);
            format!("texte saisi ({n} caractères)")
        }
    };
    Ok(report)
}

/// Saisie best-effort d'une chaîne : décompose chaque caractère en appui/relâche
/// via la façade `input::key` (gère la touche Shift multiplateforme).
pub(crate) fn type_text(hwnd: Option<isize>, text: &str) -> usize {
    let mut count = 0;
    for ch in text.chars() {
        let Some((code, shift)) = char_to_code(ch) else {
            tracing::debug!("caractère non saisissable ignoré: {ch:?}");
            continue;
        };
        if shift {
            input::key(hwnd, "ShiftLeft", true);
        }
        input::key(hwnd, &code, true);
        input::key(hwnd, &code, false);
        if shift {
            input::key(hwnd, "ShiftLeft", false);
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

/// Capture une seule frame JPEG de la fenêtre via une session temporaire.
pub(crate) async fn capture_frame(window_id: isize, show_cursor: bool) -> Result<Vec<u8>, String> {
    let (session, mut rx) = capture::start(window_id, 80, 1.0, show_cursor)
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

    let _ = session.stop();

    if frame_bytes.is_empty() {
        return Err("délai dépassé pour recevoir la frame de capture".into());
    }
    Ok(frame_bytes)
}

async fn tool_screenshot(state: &AppState) -> Result<String, String> {
    let target = *state.attached.lock().unwrap();
    let Some(target) = target else {
        return Err("aucune fenêtre attachée (appeler attach_to_window d'abord)".into());
    };
    let frame = capture_frame(target.id, true).await?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&frame))
}

fn tool_launch(args: Value, state: &AppState) -> Result<String, String> {
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

    #[allow(unused_variables)]
    let allow_virtual = {
        let cfg_opt = state.config.lock().unwrap();
        if let Some(cfg_mutex) = &*cfg_opt {
            cfg_mutex.lock().unwrap().allow_virtual_desktop
        } else {
            false
        }
    };

    #[cfg(windows)]
    if allow_virtual {
        if let Some(_hdesk) = crate::input::desktop::get_or_create_virtual_desktop() {
            match crate::input::desktop::spawn_on_desktop(command, &arguments, "WinSta0\\appMirror_Desktop") {
                Ok(pid) => return Ok(format!("application lancée avec succès sur le bureau virtuel (PID: {pid})")),
                Err(e) => return Err(format!("impossible de lancer l'application sur le bureau virtuel : {e}")),
            }
        }
    }

    // Lance le processus en tâche de fond (bureau par défaut)
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

fn attached_context(state: &AppState) -> Result<String, String> {
    let target = *state.attached.lock().unwrap();
    let Some(target) = target else {
        return Err("aucune fenêtre attachée (appeler attach_to_window d'abord)".into());
    };
    let (locked_apps, allow_virtual) = {
        let cfg_opt = state.config.lock().unwrap();
        if let Some(cfg_mutex) = &*cfg_opt {
            let cfg = cfg_mutex.lock().unwrap();
            (cfg.locked_apps.clone(), cfg.allow_virtual_desktop)
        } else { (Vec::new(), false) }
    };
    capture::list_windows(&locked_apps, allow_virtual)
        .map_err(|e| format!("liste des fenêtres: {e}"))?
        .into_iter()
        .find(|window| window.id == target.id)
        .map(|window| window.context.to_lowercase())
        .ok_or_else(|| "contexte de la fenêtre attachée introuvable".to_string())
}

async fn tool_list_layouts(args: Value, state: &AppState) -> Result<serde_json::Value, String> {
    let parsed: ListLayoutsArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let context = match parsed.context {
        Some(value) => value,
        None => attached_context(state)?,
    };
    let layouts = crate::layout_store::list(Some(&context)).await?;
    serde_json::to_value(layouts).map_err(|e| format!("sérialisation layouts: {e}"))
}

async fn tool_get_layout(args: Value, state: &AppState) -> Result<serde_json::Value, String> {
    let parsed: GetLayoutArgs = serde_json::from_value(args).map_err(|e| format!("arguments invalides: {e}"))?;
    let context = match parsed.context {
        Some(value) => value,
        None => attached_context(state)?,
    };
    if let Some(name) = parsed.name {
        let layout = crate::layout_store::load_named(&context, &name).await?;
        Ok(layout_agent_view(&layout))
    } else {
        let layouts = crate::layout_store::load_context(&context).await?;
        Ok(json!({
            "context": context,
            "layouts": layouts.iter().map(layout_agent_view).collect::<Vec<_>>(),
        }))
    }
}

/// Cible + layouts du contexte, mutualisés par les outils d'activation.
async fn activation_inputs(args: &Value, state: &AppState) -> Result<(crate::app::AttachedWindow, Vec<LayoutFile>), String> {
    let target = *state.attached.lock().unwrap();
    let Some(target) = target else {
        return Err("aucune fenêtre attachée (appeler attach_to_window d'abord)".into());
    };
    let context = match args.get("context").and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => attached_context(state)?,
    };
    let layouts = crate::layout_store::load_context(&context).await?;
    Ok((target, layouts))
}

async fn tool_activate(args: Value, state: &AppState) -> Result<Value, String> {
    let (target, layouts) = activation_inputs(&args, state).await?;
    let element = args
        .get("element")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'element' manquant".to_string())?;
    let layout = args.get("layout").and_then(Value::as_str);
    let strategy = crate::activate::Strategy::parse(args.get("strategy").and_then(Value::as_str))?;
    let verify = args.get("verify").and_then(Value::as_bool).unwrap_or(true);
    let context = layouts.first().map(|l| l.context.clone());
    let states = match &context {
        Some(context) => crate::state_store::load_context(context).await.unwrap_or_default(),
        None => Vec::new(),
    };
    crate::activate::activate_element(target, &layouts, &states, layout, element, strategy, verify, None).await
}

/// Contexte + états enregistrés, mutualisés par les outils de détection.
async fn detection_inputs(args: &Value, state: &AppState) -> Result<(crate::app::AttachedWindow, String, Vec<crate::protocol::UiStateFile>), String> {
    let target = *state.attached.lock().unwrap();
    let Some(target) = target else {
        return Err("aucune fenêtre attachée (appeler attach_to_window d'abord)".into());
    };
    let context = match args.get("context").and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => value.trim().to_lowercase(),
        _ => attached_context(state)?,
    };
    let states = crate::state_store::load_context(&context).await?;
    if states.is_empty() {
        return Err(format!(
            "aucun état enregistré pour le contexte '{context}'; capturez des états via le panneau Layout (bouton Capture state)"
        ));
    }
    Ok((target, context, states))
}

async fn tool_get_state(args: Value, state: &AppState) -> Result<Value, String> {
    let (target, context, states) = detection_inputs(&args, state).await?;
    let frame = capture_frame(target.id, false).await?;
    let (best, others) = crate::detect::detect(&frame, &states)?;
    Ok(json!({
        "context": context,
        "state": best.as_ref().map(|s| s.name.clone()),
        "detected": best.as_ref().map(crate::detect::score_view),
        "otherCandidates": others.iter().map(crate::detect::score_view).collect::<Vec<_>>(),
    }))
}

async fn tool_detect_layout(args: Value, state: &AppState) -> Result<Value, String> {
    let (target, context, states) = detection_inputs(&args, state).await?;
    let frame = capture_frame(target.id, false).await?;
    let (best, others) = crate::detect::detect(&frame, &states)?;
    let Some(best) = best else {
        return Err(format!(
            "aucun état reconnu avec assez de confiance; candidats: {}",
            serde_json::to_string(&others.iter().map(crate::detect::score_view).collect::<Vec<_>>()).unwrap_or_default()
        ));
    };
    let layout = best.layout.clone().ok_or_else(|| {
        format!("état '{}' détecté mais aucun layout ne lui est associé", best.name)
    })?;
    let file = crate::layout_store::load_named(&context, &layout).await?;
    Ok(json!({
        "context": context,
        "detected": crate::detect::score_view(&best),
        "layout": layout_agent_view(&file),
    }))
}

async fn tool_wait_state(args: Value, state: &AppState) -> Result<Value, String> {
    let (target, context, states) = detection_inputs(&args, state).await?;
    let wanted = args
        .get("state")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'state' manquant".to_string())?;
    if !states.iter().any(|s| s.name == wanted) {
        let known: Vec<&str> = states.iter().map(|s| s.name.as_str()).collect();
        return Err(format!("état '{wanted}' inconnu pour '{context}'; états connus: {}", known.join(", ")));
    }
    let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(5_000).clamp(250, 60_000);
    let started = std::time::Instant::now();

    // Une seule session de capture pour tout le sondage (latest-frame-wins).
    let (session, rx) = capture::start(target.id, 80, 1.0, false)
        .map_err(|e| format!("impossible de démarrer la capture: {e}"))?;
    let mut polls = 0usize;
    let mut last: Option<Value> = None;
    let result = loop {
        let frame = rx.borrow().clone();
        if !frame.is_empty() {
            polls += 1;
            let (best, others) = crate::detect::detect(&frame, &states)?;
            let matched = best.as_ref().is_some_and(|s| s.name == wanted);
            last = best
                .as_ref()
                .map(crate::detect::score_view)
                .or_else(|| others.first().map(crate::detect::score_view));
            if matched {
                break Ok(json!({
                    "context": context,
                    "state": wanted,
                    "detected": best.as_ref().map(crate::detect::score_view),
                    "elapsedMs": started.elapsed().as_millis() as u64,
                    "polls": polls,
                }));
            }
        }
        if started.elapsed().as_millis() as u64 >= timeout_ms {
            break Err(format!(
                "état '{wanted}' non détecté après {timeout_ms} ms ({polls} relevés); dernier candidat: {}",
                last.map(|v| v.to_string()).unwrap_or_else(|| "aucun".into())
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    };
    let _ = session.stop();
    result
}

async fn tool_type_into(args: Value, state: &AppState) -> Result<Value, String> {
    let (target, layouts) = activation_inputs(&args, state).await?;
    let element = args
        .get("element")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'element' manquant".to_string())?;
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "paramètre 'text' manquant".to_string())?;
    let layout = args.get("layout").and_then(Value::as_str);
    crate::activate::type_into_element(target, &layouts, layout, element, text).await
}

fn layout_agent_view(layout: &LayoutFile) -> Value {
    let zones = layout.zones.iter().map(|zone| {
        let bounds = zone.bounds;
        let suggested_action = match zone.kind {
            LayoutZoneKind::Input | LayoutZoneKind::Textarea => "click_then_text",
            LayoutZoneKind::Button
            | LayoutZoneKind::Dropdown
            | LayoutZoneKind::Menu
            | LayoutZoneKind::Checkbox
            | LayoutZoneKind::Radio
            | LayoutZoneKind::Link => "click",
            LayoutZoneKind::Text | LayoutZoneKind::Region => "inspect",
        };
        json!({
            "id": zone.id,
            "name": zone.name,
            "kind": zone.kind,
            "label": zone.label,
            "description": zone.description,
            "bounds": bounds,
            "center": {
                "x": bounds.x + bounds.width / 2.0,
                "y": bounds.y + bounds.height / 2.0,
            },
            "activation": zone.activation,
            "expectedState": zone.expected_state,
            "anchor": zone.anchor,
            "suggestedAction": suggested_action,
        })
    }).collect::<Vec<_>>();
    json!({
        "version": layout.version,
        "context": layout.context,
        "name": layout.name,
        "sourceWidth": layout.source_width,
        "sourceHeight": layout.source_height,
        "coordinateSpace": "normalized_0_1",
        "zones": zones,
    })
}

async fn tool_inspect(state: &AppState) -> Result<serde_json::Value, String> {
    let target = *state.attached.lock().unwrap();
    let Some(target) = target else {
        return Err("aucune fenêtre attachée (appeler attach_to_window d'abord)".into());
    };
    let context = attached_context(state)?;
    let native_elements = input::inspect_ui_elements(target.id)?;
    let user_layouts = crate::layout_store::load_context(&context).await?;
    Ok(json!({
        "context": context,
        "nativeElements": native_elements,
        "userLayouts": user_layouts.iter().map(layout_agent_view).collect::<Vec<_>>(),
    }))
}
