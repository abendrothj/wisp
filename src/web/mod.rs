pub mod ws;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rust_embed::RustEmbed;
use tokio::{sync::{mpsc, oneshot, watch}, time::Duration};
use serde::{Deserialize, Serialize};

use crate::telemetry::Snapshot;
use crate::{RemoteAction, RemoteActionRequest, RemoteActionResult};

/// Static assets compiled into the binary via rust-embed.
#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

#[derive(Clone)]
pub struct WebState {
    pub snapshot_rx: watch::Receiver<Option<Snapshot>>,
    pub action_tx: mpsc::Sender<RemoteActionRequest>,
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws::handler))
        .route("/api/action/start", post(start_handler))
        .route("/api/action/stop", post(stop_handler))
        .route("/api/action/restart", post(restart_handler))
        .route("/api/action/logs", post(logs_handler))
        .route("/api/action/inspect", post(inspect_handler))
        .route("/api/action/prune", post(prune_handler))
        .route("/api/action/system-df", post(system_df_handler))
        .with_state(state)
}

#[derive(Deserialize)]
struct NameActionRequest {
    name: String,
}

#[derive(Serialize)]
struct ActionResponse {
    title: String,
    output: String,
    is_error: bool,
}

async fn restart_handler(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<NameActionRequest>,
) -> Response {
    if !is_authorized_action_request(&headers) {
        return unauthorized_action_response();
    }
    run_action(state.action_tx.clone(), RemoteAction::Restart { name: req.name }).await
}

async fn start_handler(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<NameActionRequest>,
) -> Response {
    if !is_authorized_action_request(&headers) {
        return unauthorized_action_response();
    }
    run_action(state.action_tx.clone(), RemoteAction::Start { name: req.name }).await
}

async fn stop_handler(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<NameActionRequest>,
) -> Response {
    if !is_authorized_action_request(&headers) {
        return unauthorized_action_response();
    }
    run_action(state.action_tx.clone(), RemoteAction::Stop { name: req.name }).await
}

async fn logs_handler(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<NameActionRequest>,
) -> Response {
    if !is_authorized_action_request(&headers) {
        return unauthorized_action_response();
    }
    run_action(state.action_tx.clone(), RemoteAction::Logs { name: req.name }).await
}

async fn inspect_handler(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<NameActionRequest>,
) -> Response {
    if !is_authorized_action_request(&headers) {
        return unauthorized_action_response();
    }
    run_action(state.action_tx.clone(), RemoteAction::Inspect { name: req.name }).await
}

async fn system_df_handler(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if !is_authorized_action_request(&headers) {
        return unauthorized_action_response();
    }
    run_action(state.action_tx.clone(), RemoteAction::SystemDf).await
}

async fn prune_handler(State(state): State<WebState>, headers: HeaderMap) -> Response {
    if !is_authorized_action_request(&headers) {
        return unauthorized_action_response();
    }
    run_action(state.action_tx.clone(), RemoteAction::Prune).await
}

fn is_authorized_action_request(headers: &HeaderMap) -> bool {
    headers
        .get("x-wisp-action")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "1")
        .unwrap_or(false)
}

fn unauthorized_action_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ActionResponse {
            title: "Action blocked".to_string(),
            output: "missing or invalid x-wisp-action header".to_string(),
            is_error: true,
        }),
    ).into_response()
}

async fn run_action(action_tx: mpsc::Sender<RemoteActionRequest>, action: RemoteAction) -> Response {
    let (tx, rx) = oneshot::channel::<RemoteActionResult>();
    let request = RemoteActionRequest { action, respond_to: tx };

    if action_tx.send(request).await.is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ActionResponse {
                title: "Action unavailable".to_string(),
                output: "background action worker is not available".to_string(),
                is_error: true,
            }),
        ).into_response();
    }

    match tokio::time::timeout(Duration::from_secs(35), rx).await {
        Ok(Ok(result)) => {
            let status = if result.is_error {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::OK
            };
            (
                status,
                Json(ActionResponse {
                    title: result.title,
                    output: result.output,
                    is_error: result.is_error,
                }),
            ).into_response()
        }
        Ok(Err(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ActionResponse {
                title: "Action failed".to_string(),
                output: "action result channel closed".to_string(),
                is_error: true,
            }),
        ).into_response(),
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(ActionResponse {
                title: "Action timed out".to_string(),
                output: "remote command timed out".to_string(),
                is_error: true,
            }),
        ).into_response(),
    }
}

async fn index_handler() -> impl IntoResponse {
    serve_asset("index.html")
}

fn serve_asset(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref())],
                file.data.into_owned(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
