use std::collections::HashMap;

use serde_json::{Value, json};

use crate::{
    error::{ActionError, RETCODE_INTERNAL, RETCODE_NOT_FOUND},
    fanout::{build_message_event, fanout_group_message, fanout_private_message},
    model::{
        Account, ActionRequest, ChatEvent, InternalAction, MessageSegment, SessionType,
        private_session_id,
    },
    state::AppState,
    store::StoreError,
};

pub async fn execute(
    state: &AppState,
    actor: &Account,
    request: ActionRequest,
) -> Result<Value, ActionError> {
    match request.action {
        InternalAction::SendPrivateMsg { user_id, message } => {
            send_private(state, actor, user_id, message).await
        }
        InternalAction::SendGroupMsg { group_id, message } => {
            send_group(state, actor, group_id, message).await
        }
        InternalAction::SendMsg {
            message_type,
            user_id,
            group_id,
            message,
        } => {
            let target = resolve_message_target(message_type, user_id, group_id)?;
            match target {
                Target::Private(user_id) => send_private(state, actor, user_id, message).await,
                Target::Group(group_id) => send_group(state, actor, group_id, message).await,
            }
        }
        InternalAction::GetLoginInfo => Ok(json!({
            "user_id": actor.id,
            "nickname": actor.username,
        })),
        InternalAction::GetGroupList => get_group_list(state, actor).await,
        InternalAction::GetGroupMemberList { group_id } => {
            get_group_member_list(state, actor, group_id).await
        }
        InternalAction::GetGroupMemberInfo { group_id, user_id } => {
            get_group_member_info(state, actor, group_id, user_id).await
        }
        InternalAction::GetFriendList => get_friend_list(state, actor).await,
        InternalAction::GetGroupMsgHistory {
            group_id,
            before_seq,
            after_seq,
            limit,
        } => get_group_history(state, actor, group_id, before_seq, after_seq, limit).await,
        InternalAction::GetFriendMsgHistory {
            user_id,
            before_seq,
            after_seq,
            limit,
        } => get_friend_history(state, actor, user_id, before_seq, after_seq, limit).await,
        InternalAction::Unsupported { name } => Err(ActionError::unsupported(&name)),
    }
}

enum Target {
    Private(i64),
    Group(i64),
}

fn resolve_message_target(
    message_type: Option<String>,
    user_id: Option<i64>,
    group_id: Option<i64>,
) -> Result<Target, ActionError> {
    match message_type.as_deref() {
        Some("private") => user_id
            .map(Target::Private)
            .ok_or_else(|| ActionError::invalid("send_msg private requires user_id")),
        Some("group") => group_id
            .map(Target::Group)
            .ok_or_else(|| ActionError::invalid("send_msg group requires group_id")),
        Some(other) => Err(ActionError::invalid(format!(
            "unsupported message_type {other}"
        ))),
        None => match (user_id, group_id) {
            (Some(user_id), None) => Ok(Target::Private(user_id)),
            (None, Some(group_id)) => Ok(Target::Group(group_id)),
            (None, None) => Err(ActionError::invalid(
                "send_msg requires user_id, group_id, or message_type",
            )),
            (Some(_), Some(_)) => Err(ActionError::invalid(
                "send_msg requires message_type when both user_id and group_id are present",
            )),
        },
    }
}

async fn send_private(
    state: &AppState,
    actor: &Account,
    peer_id: i64,
    message: Vec<MessageSegment>,
) -> Result<Value, ActionError> {
    validate_target_id(peer_id)?;
    validate_message(&message)?;
    if peer_id == actor.id {
        return Err(ActionError::invalid(
            "cannot send a private message to yourself",
        ));
    }
    state
        .store
        .get_account(peer_id)
        .await
        .map_err(map_store_error)?;

    let session_id = private_session_id(actor.id, peer_id);
    let stored = state
        .store
        .create_message(SessionType::Private, &session_id, actor.id, message)
        .await
        .map_err(map_store_error)?;

    if let Err(err) = fanout_private_message(state, &stored, actor.id, peer_id).await {
        tracing::error!(error = %err, message_id = stored.id, "private message fanout failed after commit");
    }
    Ok(json!({
        "message_id": stored.id,
        "message_seq": stored.message_seq,
    }))
}

async fn send_group(
    state: &AppState,
    actor: &Account,
    group_id: i64,
    message: Vec<MessageSegment>,
) -> Result<Value, ActionError> {
    validate_target_id(group_id)?;
    validate_message(&message)?;
    state
        .store
        .get_member(group_id, actor.id)
        .await
        .map_err(|err| match err {
            StoreError::NotFound(_) => ActionError::forbidden("not a member of this group"),
            other => map_store_error(other),
        })?;

    let stored = state
        .store
        .create_message(SessionType::Group, &group_id.to_string(), actor.id, message)
        .await
        .map_err(map_store_error)?;

    if let Err(err) = fanout_group_message(state, &stored).await {
        tracing::error!(error = %err, message_id = stored.id, "group message fanout failed after commit");
    }
    Ok(json!({
        "message_id": stored.id,
        "message_seq": stored.message_seq,
    }))
}

async fn get_group_list(state: &AppState, actor: &Account) -> Result<Value, ActionError> {
    let groups = state
        .store
        .list_groups_for(actor.id)
        .await
        .map_err(map_store_error)?;
    let data: Vec<Value> = groups
        .into_iter()
        .map(|group| {
            json!({
                "group_id": group.id,
                "group_name": group.name,
                "member_count": group.member_count,
                "max_member_count": 200,
                "owner_id": group.owner_id,
                "role": group.self_role,
            })
        })
        .collect();
    Ok(Value::Array(data))
}

async fn get_group_member_list(
    state: &AppState,
    actor: &Account,
    group_id: i64,
) -> Result<Value, ActionError> {
    validate_target_id(group_id)?;
    state
        .store
        .get_member(group_id, actor.id)
        .await
        .map_err(|err| match err {
            StoreError::NotFound(_) => ActionError::forbidden("not a member of this group"),
            other => map_store_error(other),
        })?;
    let members = state
        .store
        .list_members(group_id)
        .await
        .map_err(map_store_error)?;
    let data: Vec<Value> = members
        .into_iter()
        .map(|member| {
            json!({
                "group_id": member.group_id,
                "user_id": member.account_id,
                "nickname": member.username,
                "card": "",
                "role": member.role,
                "join_time": member.joined_at.timestamp(),
            })
        })
        .collect();
    Ok(Value::Array(data))
}

async fn get_group_member_info(
    state: &AppState,
    actor: &Account,
    group_id: i64,
    target_id: i64,
) -> Result<Value, ActionError> {
    validate_target_id(group_id)?;
    validate_target_id(target_id)?;
    state
        .store
        .get_member(group_id, actor.id)
        .await
        .map_err(|err| match err {
            StoreError::NotFound(_) => ActionError::forbidden("not a member of this group"),
            other => map_store_error(other),
        })?;
    let member = state
        .store
        .get_member(group_id, target_id)
        .await
        .map_err(map_store_error)?;
    Ok(json!({
        "group_id": member.group_id,
        "user_id": member.account_id,
        "nickname": member.username,
        "card": "",
        "role": member.role,
        "join_time": member.joined_at.timestamp(),
    }))
}

async fn get_friend_list(state: &AppState, actor: &Account) -> Result<Value, ActionError> {
    let friends = state
        .store
        .list_friends(actor.id)
        .await
        .map_err(map_store_error)?;
    let data: Vec<Value> = friends
        .into_iter()
        .map(|friend| {
            json!({
                "user_id": friend.account_id,
                "nickname": friend.username,
                "remark": "",
            })
        })
        .collect();
    Ok(Value::Array(data))
}

async fn get_group_history(
    state: &AppState,
    actor: &Account,
    group_id: i64,
    before_seq: Option<i64>,
    after_seq: Option<i64>,
    limit: Option<i64>,
) -> Result<Value, ActionError> {
    validate_target_id(group_id)?;
    state
        .store
        .get_member(group_id, actor.id)
        .await
        .map_err(|err| match err {
            StoreError::NotFound(_) => ActionError::forbidden("not a member of this group"),
            other => map_store_error(other),
        })?;
    let limit = history_limit(state, limit);
    let messages = state
        .store
        .list_messages(
            SessionType::Group,
            &group_id.to_string(),
            before_seq,
            after_seq,
            limit,
        )
        .await
        .map_err(map_store_error)?;

    let members = state.cache.group_members(group_id).await.ok();
    let unique_senders: Vec<i64> = {
        let mut ids: Vec<i64> = messages.iter().map(|m| m.sender_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let accounts = state
        .store
        .get_accounts_by_ids(&unique_senders)
        .await
        .map_err(map_store_error)?;
    let names: HashMap<i64, String> = accounts.into_iter().map(|a| (a.id, a.username)).collect();
    let roles: HashMap<i64, String> = members
        .as_ref()
        .map(|members| {
            members
                .iter()
                .map(|m| (m.account_id, m.role.clone()))
                .collect()
        })
        .unwrap_or_default();
    let group_name = state.store.get_group(group_id).await.ok().map(|g| g.name);

    let mut events = Vec::with_capacity(messages.len());
    for message in &messages {
        let sender_name = names
            .get(&message.sender_id)
            .cloned()
            .unwrap_or_else(|| format!("account-{}", message.sender_id));
        let role = roles.get(&message.sender_id).map(String::as_str);
        let mut event = build_message_event(message, &sender_name, role, group_name.as_deref());
        event.account_id = actor.id_string();
        event.session_id = Some(group_id.to_string());
        events.push(serialize_for_action(state, actor, &event)?);
    }
    Ok(json!({ "messages": events }))
}

async fn get_friend_history(
    state: &AppState,
    actor: &Account,
    peer_id: i64,
    before_seq: Option<i64>,
    after_seq: Option<i64>,
    limit: Option<i64>,
) -> Result<Value, ActionError> {
    validate_target_id(peer_id)?;
    let limit = history_limit(state, limit);
    let messages = state
        .store
        .private_messages(actor.id, peer_id, before_seq, after_seq, limit)
        .await
        .map_err(map_store_error)?;

    let mut unique_senders: Vec<i64> = messages.iter().map(|m| m.sender_id).collect();
    unique_senders.sort_unstable();
    unique_senders.dedup();
    let accounts = state
        .store
        .get_accounts_by_ids(&unique_senders)
        .await
        .map_err(map_store_error)?;
    let names: HashMap<i64, String> = accounts.into_iter().map(|a| (a.id, a.username)).collect();

    let mut events = Vec::with_capacity(messages.len());
    for message in &messages {
        let sender_name = names
            .get(&message.sender_id)
            .cloned()
            .unwrap_or_else(|| format!("account-{}", message.sender_id));
        let mut event = build_message_event(message, &sender_name, None, None);
        event.account_id = actor.id_string();
        event.session_id = Some(peer_id.to_string());
        events.push(serialize_for_action(state, actor, &event)?);
    }
    Ok(json!({ "messages": events }))
}

fn serialize_for_action(
    state: &AppState,
    actor: &Account,
    event: &ChatEvent,
) -> Result<Value, ActionError> {
    let payload = state.adapter.serialize_event(event, actor).map_err(|err| {
        tracing::error!(error = %err, event_id = %event.event_id, "history serialization failed");
        ActionError::new(RETCODE_INTERNAL, "event serialization failed")
    })?;
    serde_json::from_str(&payload).map_err(|err| {
        tracing::error!(error = %err, "serialized event was not valid JSON");
        ActionError::new(RETCODE_INTERNAL, "event serialization failed")
    })
}

fn history_limit(state: &AppState, requested: Option<i64>) -> i64 {
    requested
        .unwrap_or(state.config.history_default_limit)
        .clamp(1, state.config.history_max_limit)
}

fn validate_target_id(id: i64) -> Result<(), ActionError> {
    if id <= 0 {
        return Err(ActionError::invalid("target id must be a positive integer"));
    }
    Ok(())
}

fn validate_message(message: &[MessageSegment]) -> Result<(), ActionError> {
    if message.is_empty() {
        return Err(ActionError::invalid(
            "message must contain at least one segment",
        ));
    }
    if message.len() > 1000 {
        return Err(ActionError::invalid("message has too many segments"));
    }
    for segment in message {
        if segment.segment_type.trim().is_empty() {
            return Err(ActionError::invalid("message segment type is empty"));
        }
    }
    Ok(())
}

fn map_store_error(err: StoreError) -> ActionError {
    match err {
        StoreError::NotFound(what) => {
            ActionError::new(RETCODE_NOT_FOUND, format!("{what} not found"))
        }
        StoreError::Forbidden(message) => ActionError::forbidden(message),
        StoreError::Conflict(message) => ActionError::invalid(message),
        StoreError::UsernameTaken => ActionError::invalid("username already exists"),
        StoreError::Internal(message) => {
            tracing::error!(error = %message, "internal storage failure");
            ActionError::new(RETCODE_INTERNAL, "internal storage failure")
        }
        StoreError::Database(err) => {
            tracing::error!(error = %err, "database failure during action");
            ActionError::new(RETCODE_INTERNAL, "database failure")
        }
    }
}
