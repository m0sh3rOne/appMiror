//! Transport MCP « HTTP+SSE » (révision 2024-11-05).
//!
//! - `GET /api/mcp/sse` ouvre un flux SSE persistant (serveur -> client). Le
//!   premier évènement `endpoint` indique au client l'URL de POST, portant son
//!   `sessionId`.
//! - `POST /api/mcp/message?sessionId=…` reçoit les requêtes JSON-RPC. La
//!   réponse est **poussée sur le flux SSE** de la session (et le POST répond
//!   202 Accepted), comme l'exige ce transport.
//!
//! Repli testable : un POST **sans** `sessionId` (ou dont la session SSE est
//! morte) reçoit la réponse JSON-RPC directement dans le corps HTTP. Cela permet
//! de valider le serveur au `curl` sans établir de flux SSE au préalable.
//!
//! Liaison montant/descendant via un `tokio::sync::mpsc` par session, référencé
//! dans `AppState::mcp_sessions`.

use std::convert::Infallible;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::app::AppState;
use crate::mcp::handle_mcp_request;
use crate::protocol::McpRequest;

/// Ouvre le flux SSE descendant et enregistre la session.
pub async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel::<String>(32);
    state.mcp_sessions.lock().unwrap().insert(session_id.clone(), tx);

    // 1er évènement obligatoire : l'endpoint de POST portant le sessionId.
    let endpoint = format!("/api/mcp/message?sessionId={session_id}");
    let handshake = stream::once(async move {
        Ok(Event::default().event("endpoint").data(endpoint))
    });
    // Puis chaque réponse JSON-RPC poussée par le handler POST.
    let messages =
        ReceiverStream::new(rx).map(|data| Ok(Event::default().event("message").data(data)));

    Sse::new(handshake.chain(messages)).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
pub struct MessageQuery {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

/// Reçoit une requête JSON-RPC montante.
pub async fn message_handler(
    State(state): State<AppState>,
    Query(query): Query<MessageQuery>,
    body: Bytes,
) -> Response {
    let req: McpRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => {
            let err = json!({
                "jsonrpc": "2.0", "id": Value::Null,
                "error": { "code": -32700, "message": format!("JSON invalide: {e}") }
            });
            return (StatusCode::BAD_REQUEST, Json(err)).into_response();
        }
    };

    let Some(response) = handle_mcp_request(req, &state).await else {
        // Notification : rien à renvoyer.
        return StatusCode::ACCEPTED.into_response();
    };

    // Transport SSE nominal : pousser sur le flux de la session puis 202.
    if let Some(sid) = query.session_id {
        let sender = state.mcp_sessions.lock().unwrap().get(&sid).cloned();
        if let Some(sender) = sender {
            let payload = serde_json::to_string(&response).unwrap_or_default();
            if sender.send(payload).await.is_ok() {
                return StatusCode::ACCEPTED.into_response();
            }
            // Flux SSE fermé : on purge la session et on bascule sur le corps.
            state.mcp_sessions.lock().unwrap().remove(&sid);
        }
    }

    // Repli : réponse directe dans le corps HTTP.
    Json(response).into_response()
}
