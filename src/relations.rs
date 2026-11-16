use axum::http::StatusCode;

use crate::{
    error::{ApiError, ApiResult},
    fanout::{fanout_friend_request, fanout_group_join_request, fanout_group_notice},
    model::{
        Account, ActionableGroupJoinRequest, FriendRequestView, GroupJoinRequestView, GroupView,
        IncomingFriendRequest,
    },
    state::AppState,
    store::StoreError,
};

#[derive(Debug)]
pub struct PendingRequests {
    pub friend_requests: Vec<IncomingFriendRequest>,
    pub group_join_requests: Vec<ActionableGroupJoinRequest>,
}

pub async fn pending_requests(state: &AppState, actor: &Account) -> ApiResult<PendingRequests> {
    let (friend_requests, group_join_requests) = tokio::try_join!(
        state.store.list_incoming_friend_requests(actor.id),
        state.store.list_actionable_group_join_requests(actor.id),
    )
    .map_err(map_store_error)?;
    Ok(PendingRequests {
        friend_requests,
        group_join_requests,
    })
}

pub async fn request_friend(
    state: &AppState,
    actor: &Account,
    target_id: i64,
) -> ApiResult<FriendRequestView> {
    require_positive(target_id, "target_id")?;
    let request = state
        .store
        .create_friend_request(actor.id, target_id)
        .await
        .map_err(map_store_error)?;
    if let Err(err) = fanout_friend_request(state, &request).await {
        tracing::error!(error = %err, request_id = request.id, "friend request event fanout failed");
    }
    Ok(request)
}

pub async fn approve_friend(
    state: &AppState,
    actor: &Account,
    request_id: i64,
    approve: bool,
) -> ApiResult<FriendRequestView> {
    require_positive(request_id, "request_id")?;
    state
        .store
        .resolve_friend_request(request_id, actor.id, approve)
        .await
        .map_err(map_store_error)
}

pub async fn create_group(state: &AppState, actor: &Account, name: &str) -> ApiResult<GroupView> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 128 {
        return Err(ApiError::bad_request(
            "group name must be between 1 and 128 characters",
        ));
    }
    state
        .store
        .create_group(name, actor.id)
        .await
        .map_err(map_store_error)
}

pub async fn request_join_group(
    state: &AppState,
    actor: &Account,
    group_id: i64,
) -> ApiResult<GroupJoinRequestView> {
    require_positive(group_id, "group_id")?;
    let request = state
        .store
        .create_group_join_request(group_id, actor.id)
        .await
        .map_err(map_store_error)?;
    if let Err(err) = fanout_group_join_request(state, &request).await {
        tracing::error!(error = %err, request_id = request.id, "group join request event fanout failed");
    }
    Ok(request)
}

pub async fn approve_join(
    state: &AppState,
    actor: &Account,
    group_id: i64,
    request_id: i64,
    approve: bool,
) -> ApiResult<GroupJoinRequestView> {
    require_positive(group_id, "group_id")?;
    require_positive(request_id, "request_id")?;
    let request = state
        .store
        .resolve_group_join_request(group_id, request_id, actor.id, approve)
        .await
        .map_err(map_store_error)?;
    state.cache.invalidate_group(group_id).await;
    if approve
        && let Err(err) = fanout_group_notice(
            state,
            group_id,
            "group_increase",
            actor.id,
            request.account_id,
            "approve",
            &[],
        )
        .await
    {
        tracing::error!(error = %err, group_id, request_id, "group_increase notice fanout failed");
    }
    Ok(request)
}

pub async fn leave_group(state: &AppState, actor: &Account, group_id: i64) -> ApiResult<()> {
    require_positive(group_id, "group_id")?;
    state
        .store
        .leave_group(group_id, actor.id)
        .await
        .map_err(map_store_error)?;
    state.cache.invalidate_group(group_id).await;
    if let Err(err) = fanout_group_notice(
        state,
        group_id,
        "group_decrease",
        actor.id,
        actor.id,
        "leave",
        &[actor.id],
    )
    .await
    {
        tracing::error!(error = %err, group_id, account_id = actor.id, "group_decrease notice fanout failed");
    }
    Ok(())
}

fn require_positive(value: i64, field: &str) -> ApiResult<()> {
    if value <= 0 {
        return Err(ApiError::bad_request(format!(
            "{field} must be a positive integer"
        )));
    }
    Ok(())
}

fn map_store_error(err: StoreError) -> ApiError {
    match err {
        StoreError::UsernameTaken => ApiError::conflict("username is already taken"),
        StoreError::NotFound(what) => ApiError::not_found(format!("{what} not found")),
        StoreError::Conflict(message) => ApiError::conflict(message),
        StoreError::Forbidden(message) => ApiError::forbidden(message),
        StoreError::Internal(message) => {
            tracing::error!(error = %message, "internal storage failure");
            ApiError::internal("internal storage failure")
        }
        StoreError::Database(err) => {
            tracing::error!(error = %err, "database failure");
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "database operation failed",
            )
        }
    }
}
