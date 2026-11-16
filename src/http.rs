use std::collections::HashSet;

use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::{
    auth::{
        AuthenticatedAccount, authenticate_token, constant_time_eq, hash_token, token_fingerprint,
    },
    error::{
        ApiError, ApiResult, RETCODE_INVALID_REQUEST, RETCODE_RATE_LIMITED, RETCODE_UNAUTHORIZED,
    },
    model::ActionResult,
    relations,
    state::AppState,
    util::parse_id,
    ws::ws_handler,
};

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        // Canonical OB11 HTTP endpoint and two aliases.
        .route("/", post(universal_action))
        .route("/api", post(universal_action))
        .route("/action", post(universal_action))
        // Action name in the path, params in the body.
        .route("/api/{action}", post(path_action))
        .route("/{action}", post(path_action))
        // Custom relationship API. Approval is here, never through OB11 actions.
        .route("/friends/request", post(friend_request))
        .route("/friends/approve", post(friend_approve))
        .route("/groups", post(create_group))
        .route("/groups/{group_id}/join", post(group_join))
        .route("/groups/{group_id}/members/approve", post(group_approve))
        .route("/groups/{group_id}/leave", post(group_leave))
        .route("/me/token/reset", post(self_token_reset))
        .route("/requests", get(list_pending_requests))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .route("/health", get(health))
        .route("/client", get(client_page))
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .route("/register", post(register))
        .route("/ws", get(ws_handler))
        .route(
            "/admin/accounts/{account_id}/token/reset",
            post(admin_token_reset),
        )
        .merge(protected)
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        // The standalone single-file client may be opened from file:// or a
        // different local origin. This is a token-authenticated API with no
        // cookies, so permissive CORS does not grant access without a token.
        // Restrict this layer behind a reverse proxy in production if desired.
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn require_auth(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_bearer);
    let Some(token) = token else {
        return unauthorized_response("missing Authorization: Bearer <token>");
    };
    match authenticate_token(state.store.pool(), token).await {
        Ok(account) => {
            request.extensions_mut().insert(account);
            next.run(request).await
        }
        Err(err) => unauthorized_response(&err.0),
    }
}

fn parse_bearer(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() { None } else { Some(token) }
}

fn unauthorized_response(_message: &str) -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "status": "failed",
            "retcode": RETCODE_UNAUTHORIZED,
            "data": null,
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

// ------------------------------------------------------------------- register

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    username: String,
    #[serde(default)]
    labels: Option<Vec<String>>,
    register_key: Option<String>,
}

async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<RegisterRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<Response> {
    let Json(body) = body.map_err(|err| ApiError::bad_request(err.body_text()))?;
    let Some(expected_key) = state.config.register_key.as_deref() else {
        return Err(ApiError::registration_disabled());
    };
    let provided_key = body.register_key.as_deref().unwrap_or_default();
    if !constant_time_eq(expected_key, provided_key) {
        return Err(ApiError::forbidden("invalid register_key"));
    }

    let username = normalize_username(&body.username)?;
    let labels = normalize_labels(body.labels.unwrap_or_default())?;
    let (account, token) = state
        .store
        .create_account(&username, &labels, state.config.rate_limit_default)
        .await
        .map_err(map_register_store_error)?;
    let token_hash = hash_token(&token);
    let base = request_base_url(&state, &headers);
    let ws_base = to_ws_base(&base);
    let ws_url = format!("{ws_base}/ws");
    let ws_url_with_token = format!("{ws_url}?access_token={token}");
    let mapping = state.adapter.action_mapping();

    let response = json!({
        "account_id": account.id,
        "username": account.username,
        "token": token,
        "token_fingerprint": token_fingerprint(&token_hash),
        "http_base_url": base,
        "ws_url": ws_url,
        "ws_url_with_token": ws_url_with_token,
        "capabilities": {
            "protocol": mapping.protocol,
            "version": mapping.version,
            "message_format": "array",
            "ws_role": "event_push_only",
            "ws_auth": "query parameter access_token",
            "http_auth": "Authorization: Bearer <token>",
            "actions": mapping.implemented_actions,
            "rate_limit_per_second": account.rate_limit,
        },
        "notice": "The token is the account. It is shown once and cannot be recovered; a server operator can reset it. Keep it secret."
    });
    Ok((StatusCode::OK, Json(response)).into_response())
}

fn normalize_username(raw: &str) -> ApiResult<String> {
    let username = raw.trim().to_ascii_lowercase();
    let len = username.chars().count();
    if !(3..=32).contains(&len) {
        return Err(ApiError::bad_request(
            "username must be 3-32 characters after trimming and lowercasing",
        ));
    }
    if !username
        .chars()
        .next()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false)
    {
        return Err(ApiError::bad_request(
            "username must start with a letter or digit",
        ));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(ApiError::bad_request(
            "username may contain only ASCII letters, digits, underscore, hyphen, and dot",
        ));
    }
    Ok(username)
}

fn normalize_labels(labels: Vec<String>) -> ApiResult<Vec<String>> {
    if labels.len() > 32 {
        return Err(ApiError::bad_request("at most 32 labels are allowed"));
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for label in labels {
        let label = label.trim().to_owned();
        if label.is_empty() {
            return Err(ApiError::bad_request("labels must not be empty"));
        }
        if label.chars().count() > 64 {
            return Err(ApiError::bad_request(
                "each label must be at most 64 characters",
            ));
        }
        if seen.insert(label.clone()) {
            normalized.push(label);
        }
    }
    Ok(normalized)
}

fn map_register_store_error(err: crate::store::StoreError) -> ApiError {
    match err {
        crate::store::StoreError::UsernameTaken => ApiError::conflict("username is already taken"),
        crate::store::StoreError::Database(db) => {
            tracing::error!(error = %db, "registration database failure");
            ApiError::internal("database operation failed")
        }
        other => {
            tracing::error!(error = %other, "registration failed");
            ApiError::internal("registration failed")
        }
    }
}

fn request_base_url(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(base) = &state.config.public_base_url {
        return base.clone();
    }
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| *v == "http" || *v == "https")
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            state
                .config
                .fallback_base_url()
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .to_owned()
        });
    format!("{scheme}://{host}")
}

fn to_ws_base(base: &str) -> String {
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_owned()
    }
}

// -------------------------------------------------------------------- actions

async fn universal_action(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
    body: Bytes,
) -> Response {
    let raw = match std::str::from_utf8(&body) {
        Ok(raw) => raw,
        Err(_) => return action_failure_response(&state, RETCODE_INVALID_REQUEST),
    };
    process_action(&state, &auth, raw).await
}

async fn path_action(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
    Path(action): Path<String>,
    body: Bytes,
) -> Response {
    let value: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(_) => return action_failure_response(&state, RETCODE_INVALID_REQUEST),
        }
    };
    let payload = match value {
        Value::Object(object) => {
            if object.contains_key("action") {
                Value::Object(object)
            } else {
                json!({ "action": action, "params": Value::Object(object) })
            }
        }
        Value::Null => json!({ "action": action, "params": Value::Null }),
        _ => return action_failure_response(&state, RETCODE_INVALID_REQUEST),
    };
    process_action(&state, &auth, &payload.to_string()).await
}

async fn process_action(state: &AppState, auth: &AuthenticatedAccount, raw: &str) -> Response {
    if !state
        .limiter
        .allow(auth.account.id, auth.account.rate_limit)
    {
        tracing::warn!(account_id = auth.account.id, "action rate limit exceeded");
        return action_failure_response(state, RETCODE_RATE_LIMITED);
    }

    let request = match state.adapter.deserialize_action(raw) {
        Ok(request) => request,
        Err(err) => {
            tracing::debug!(error = %err, "action payload rejected");
            return action_failure_response(state, RETCODE_INVALID_REQUEST);
        }
    };

    match crate::actions::execute(state, &auth.account, request).await {
        Ok(data) => action_response(state, ActionResult::ok(data)),
        Err(err) => {
            tracing::debug!(
                account_id = auth.account.id,
                retcode = err.retcode,
                message = %err.message,
                "action failed"
            );
            action_failure_response(state, err.retcode)
        }
    }
}

fn action_failure_response(state: &AppState, retcode: i32) -> Response {
    action_response(state, ActionResult::fail(retcode))
}

fn action_response(state: &AppState, result: ActionResult) -> Response {
    match state.adapter.serialize_action_response(&result) {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = %err, "failed to serialize OB11 action response");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "response serialization failed"})),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------- relationships/custom

#[derive(Debug, Deserialize)]
struct TargetRequest {
    target_id: Value,
}

async fn friend_request(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
    body: Result<Json<TargetRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(body) = body.map_err(|err| ApiError::bad_request(err.body_text()))?;
    let target_id = parse_id(&body.target_id)
        .ok_or_else(|| ApiError::bad_request("target_id must be an integer or numeric string"))?;
    let request = relations::request_friend(&state, &auth.account, target_id).await?;
    Ok(Json(json!({
        "request_id": request.id,
        "status": request.status,
        "flag": request.id.to_string(),
        "from_id": request.from_id,
        "to_id": request.to_id,
    })))
}

#[derive(Debug, Deserialize)]
struct ApproveRequest {
    request_id: Option<Value>,
    flag: Option<String>,
    approve: bool,
}

impl ApproveRequest {
    fn resolved_id(&self) -> ApiResult<i64> {
        if let Some(value) = &self.request_id {
            return parse_id(value)
                .ok_or_else(|| ApiError::bad_request("request_id must be an integer"));
        }
        if let Some(flag) = &self.flag {
            return flag
                .parse::<i64>()
                .map_err(|_| ApiError::bad_request("flag must be a numeric request id"));
        }
        Err(ApiError::bad_request("request_id or flag is required"))
    }
}

async fn friend_approve(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
    body: Result<Json<ApproveRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(body) = body.map_err(|err| ApiError::bad_request(err.body_text()))?;
    let request_id = body.resolved_id()?;
    let request =
        relations::approve_friend(&state, &auth.account, request_id, body.approve).await?;
    Ok(Json(json!({
        "request_id": request.id,
        "status": request.status,
        "from_id": request.from_id,
        "to_id": request.to_id,
    })))
}

#[derive(Debug, Deserialize)]
struct CreateGroupRequest {
    name: String,
}

async fn create_group(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
    body: Result<Json<CreateGroupRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(body) = body.map_err(|err| ApiError::bad_request(err.body_text()))?;
    let group = relations::create_group(&state, &auth.account, &body.name).await?;
    Ok(Json(json!({
        "group_id": group.id,
        "name": group.name,
        "owner_id": group.owner_id,
        "role": group.self_role,
    })))
}

async fn group_join(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
    Path(group_id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let request = relations::request_join_group(&state, &auth.account, group_id).await?;
    Ok(Json(json!({
        "request_id": request.id,
        "flag": request.id.to_string(),
        "group_id": request.group_id,
        "account_id": request.account_id,
        "status": request.status,
    })))
}

async fn group_approve(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
    Path(group_id): Path<i64>,
    body: Result<Json<ApproveRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<Json<Value>> {
    let Json(body) = body.map_err(|err| ApiError::bad_request(err.body_text()))?;
    let request_id = body.resolved_id()?;
    let request =
        relations::approve_join(&state, &auth.account, group_id, request_id, body.approve).await?;
    Ok(Json(json!({
        "request_id": request.id,
        "group_id": request.group_id,
        "account_id": request.account_id,
        "status": request.status,
    })))
}

async fn group_leave(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
    Path(group_id): Path<i64>,
) -> ApiResult<Json<Value>> {
    relations::leave_group(&state, &auth.account, group_id).await?;
    Ok(Json(json!({
        "group_id": group_id,
        "left": true,
    })))
}

// -------------------------------------------------------------- token rotation

async fn self_token_reset(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
) -> ApiResult<Json<Value>> {
    let actor = auth.account.username.clone();
    let (account, token) = state
        .store
        .rotate_token(auth.account.id, &actor, Some("self-service token reset"))
        .await
        .map_err(map_rotate_error)?;
    let kicked = state.registry.kick_account(account.id);
    let token_hash = hash_token(&token);
    let base = state.config.fallback_base_url();
    let ws_url = format!("{}/ws?access_token={token}", to_ws_base(&base));
    Ok(Json(json!({
        "account_id": account.id,
        "token": token,
        "token_fingerprint": token_fingerprint(&token_hash),
        "ws_url_with_token": ws_url,
        "revoked_connections": kicked,
        "notice": "The previous token is invalid immediately. This token is shown once.",
    })))
}

async fn admin_token_reset(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let Some(expected) = state.config.admin_key.as_deref() else {
        return Err(ApiError::forbidden(
            "admin token reset is disabled because ADMIN_KEY is not configured",
        ));
    };
    let provided = headers
        .get("x-admin-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !constant_time_eq(expected, provided) {
        return Err(ApiError::forbidden("invalid X-Admin-Key"));
    }
    let actor = headers
        .get("x-admin-actor")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("server-admin")
        .chars()
        .take(128)
        .collect::<String>();

    if account_id <= 0 {
        return Err(ApiError::bad_request("account_id must be positive"));
    }
    let (account, token) = state
        .store
        .rotate_token(
            account_id,
            &format!("admin:{actor}"),
            Some("administrator token reset"),
        )
        .await
        .map_err(map_rotate_error)?;
    let kicked = state.registry.kick_account(account.id);
    let token_hash = hash_token(&token);
    let base = state.config.fallback_base_url();
    let ws_url = format!("{}/ws?access_token={token}", to_ws_base(&base));
    Ok(Json(json!({
        "account_id": account.id,
        "username": account.username,
        "token": token,
        "token_fingerprint": token_fingerprint(&token_hash),
        "ws_url_with_token": ws_url,
        "revoked_connections": kicked,
        "actor": actor,
        "notice": "The previous token is invalid immediately. This token is shown once.",
    })))
}

fn map_rotate_error(err: crate::store::StoreError) -> ApiError {
    match err {
        crate::store::StoreError::NotFound(_) => ApiError::not_found("account not found"),
        crate::store::StoreError::Database(db) => {
            tracing::error!(error = %db, "token rotation database failure");
            ApiError::internal("database operation failed")
        }
        other => {
            tracing::error!(error = %other, "token rotation failed");
            ApiError::internal("token rotation failed")
        }
    }
}

// ------------------------------------------------------------------ web client

async fn client_page() -> Html<&'static str> {
    Html(include_str!("../client.html"))
}

// ------------------------------------------------------------ pending requests

async fn list_pending_requests(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedAccount>,
) -> ApiResult<Json<Value>> {
    let pending = relations::pending_requests(&state, &auth.account).await?;
    let friend_requests: Vec<Value> = pending
        .friend_requests
        .into_iter()
        .map(|request| {
            json!({
                "request_id": request.request_id,
                "flag": request.request_id.to_string(),
                "from_id": request.from_id,
                "from_username": request.from_username,
                "created_at": request.created_at.to_rfc3339(),
            })
        })
        .collect();
    let group_join_requests: Vec<Value> = pending
        .group_join_requests
        .into_iter()
        .map(|request| {
            json!({
                "request_id": request.request_id,
                "flag": request.request_id.to_string(),
                "group_id": request.group_id,
                "group_name": request.group_name,
                "account_id": request.account_id,
                "account_username": request.account_username,
                "created_at": request.created_at.to_rfc3339(),
            })
        })
        .collect();
    Ok(Json(json!({
        "friend_requests": friend_requests,
        "group_join_requests": group_join_requests,
    })))
}

// ---------------------------------------------------------------------- health

async fn health(State(state): State<AppState>) -> Response {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(state.store.pool())
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "database": "up",
                "active_connections": state.registry.active_connections(),
                "slow_disconnects": state.registry.slow_disconnects(),
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = %err, "health check database failure");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"ok": false, "database": "down"})),
            )
                .into_response()
        }
    }
}
