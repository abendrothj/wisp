pub mod ws;

use axum::{
    Router,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rust_embed::RustEmbed;
use tokio::sync::watch;

use crate::telemetry::Snapshot;

/// Static assets compiled into the binary via rust-embed.
#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

#[derive(Clone)]
pub struct WebState {
    pub snapshot_rx: watch::Receiver<Option<Snapshot>>,
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws::handler))
        .with_state(state)
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
