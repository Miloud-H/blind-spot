use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use tokio::sync::broadcast::error::RecvError;
use crate::AppState;

pub async fn events(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut rx = state.event_bus.subscribe();
    loop {
        match rx.recv().await {
            Ok(msg)                    => { if socket.send(Message::Text(msg)).await.is_err() { break; } }
            Err(RecvError::Lagged(_))  => continue,
            Err(RecvError::Closed)     => break,
        }
    }
}
