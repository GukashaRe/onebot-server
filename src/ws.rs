use std::time::Duration;

use axum::{
    Json,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    auth::{AuthenticatedAccount, authenticate_token, token_still_valid},
    registry::ConnectionHandle,
    state::AppState,
};

const WS_SEND_TIMEOUT: Duration = Duration::from_secs(10);
const WS_PING_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
pub struct WsAuthQuery {
    access_token: Option<String>,
}

pub async fn ws_handler(
    State(state): State<AppState>,
    Query(query): Query<WsAuthQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(token) = query.access_token else {
        return ws_unauthorized("missing access_token query parameter");
    };
    match authenticate_token(state.store.pool(), &token).await {
        Ok(auth) => ws
            .max_message_size(64 * 1024)
            .max_frame_size(64 * 1024)
            .on_upgrade(move |socket| handle_socket(state, auth, socket))
            .into_response(),
        Err(err) => ws_unauthorized(&err.0),
    }
}

fn ws_unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": {
                "code": "unauthorized",
                "message": message,
            }
        })),
    )
        .into_response()
}

async fn handle_socket(state: AppState, auth: AuthenticatedAccount, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let connection_id = state.registry.next_connection_id();
    let cancel = CancellationToken::new();
    let (tx, mut rx) = mpsc::channel::<String>(state.config.ws_queue_capacity);
    let handle = ConnectionHandle {
        connection_id,
        account_id: auth.account.id,
        sender: tx,
        cancel: cancel.clone(),
        slow: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    state.registry.register(handle);
    tracing::info!(
        account_id = auth.account.id,
        connection_id,
        "WebSocket connected"
    );

    let mut clean_exit = true;
    // First-frame events (OB12 meta.connect in the future) are delivered only
    // after the connection is registered so no fanout event can overtake them.
    match state.adapter.initial_events(&auth.account) {
        Ok(events) => {
            for event in events {
                if send_text(&mut sink, event).await.is_err() {
                    clean_exit = false;
                    break;
                }
            }
        }
        Err(err) => {
            tracing::error!(account_id = auth.account.id, error = %err, "initial events failed");
            clean_exit = false;
        }
    }

    let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut revalidate_interval = tokio::time::interval(state.config.ws_revalidate_interval);
    revalidate_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the immediate first ticks; the handshake already authenticated.
    ping_interval.tick().await;
    revalidate_interval.tick().await;

    while clean_exit {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                tracing::info!(
                    account_id = auth.account.id,
                    connection_id,
                    "WebSocket closing after cancellation (token reset or slow consumer)"
                );
                break;
            }
            maybe_payload = rx.recv() => {
                match maybe_payload {
                    Some(payload) => {
                        if send_text(&mut sink, payload).await.is_err() {
                            clean_exit = false;
                        }
                    }
                    None => break,
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if send_with_timeout(&mut sink, Message::Pong(payload)).await.is_err() {
                            clean_exit = false;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Text(_))) => {
                        // WS is event-push-only. Actions are accepted exclusively
                        // through HTTP POST; inbound action JSON is ignored.
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Err(err)) => {
                        tracing::debug!(connection_id, error = %err, "WebSocket read error");
                        clean_exit = false;
                    }
                }
            }
            _ = ping_interval.tick() => {
                if send_with_timeout(&mut sink, Message::Ping(Vec::new().into())).await.is_err() {
                    clean_exit = false;
                }
            }
            _ = revalidate_interval.tick() => {
                match token_still_valid(
                    state.store.pool(),
                    auth.account.id,
                    &auth.token_hash,
                ).await {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::info!(
                            account_id = auth.account.id,
                            connection_id,
                            "WebSocket token revalidation failed; disconnecting"
                        );
                        clean_exit = false;
                    }
                    Err(err) => {
                        // A transient database failure must not churn all live
                        // connections; proactive reset kicks still apply.
                        tracing::warn!(connection_id, error = %err, "WS token revalidation query failed");
                    }
                }
            }
        }
    }

    state.registry.unregister(auth.account.id, connection_id);
    if clean_exit {
        let _ = tokio::time::timeout(WS_SEND_TIMEOUT, sink.send(Message::Close(None))).await;
    }
    tracing::info!(
        account_id = auth.account.id,
        connection_id,
        "WebSocket disconnected"
    );
}

async fn send_text(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    payload: String,
) -> Result<(), ()> {
    send_with_timeout(sink, Message::Text(payload.into())).await
}

async fn send_with_timeout(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: Message,
) -> Result<(), ()> {
    match tokio::time::timeout(WS_SEND_TIMEOUT, sink.send(message)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => {
            tracing::debug!(error = %err, "WebSocket write failed");
            Err(())
        }
        Err(_) => {
            tracing::warn!("WebSocket write timed out; slow consumer");
            Err(())
        }
    }
}
