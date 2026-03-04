use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use tokio::sync::watch;
use tracing::debug;

use crate::telemetry::Snapshot;
use super::WebState;

pub async fn handler(ws: WebSocketUpgrade, State(state): State<WebState>) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state.snapshot_rx))
}

async fn handle_socket(mut socket: WebSocket, mut rx: watch::Receiver<Option<Snapshot>>) {
    // Send the current snapshot immediately on connect so the browser doesn't
    // wait for the next poll interval before seeing data.
    {
        let current = rx.borrow_and_update().clone();
        if let Some(snap) = current {
            if let Ok(json) = serde_json::to_string(&snap) {
                if socket.send(Message::Text(json.into())).await.is_err() {
                    return;
                }
            }
        }
    }

    // Stream every subsequent update.
    loop {
        match rx.changed().await {
            Ok(()) => {
                let snap = rx.borrow_and_update().clone();
                if let Some(snap) = snap {
                    match serde_json::to_string(&snap) {
                        Ok(json) => {
                            if socket.send(Message::Text(json.into())).await.is_err() {
                                debug!("ws client disconnected");
                                break;
                            }
                        }
                        Err(e) => debug!("snapshot serialize error: {e}"),
                    }
                }
            }
            Err(_) => break, // watch sender dropped — server shutting down
        }
    }
}
