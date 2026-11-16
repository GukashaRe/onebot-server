use std::collections::HashSet;

use chrono::Utc;

use crate::{
    error::AdapterError,
    model::{
        Account, ChatEvent, EventExtras, EventType, FriendRequestView, GroupJoinRequestView,
        GroupMemberView, SessionType, StoredMessage,
    },
    state::AppState,
    util::{new_event_id, unix_seconds},
};

/// Build the neutral message event shared by fanout and history responses.
/// For private messages the per-receiver session_id is the peer and is filled by
/// the caller/fanout; storage uses the canonical ordered pair.
pub fn build_message_event(
    message: &StoredMessage,
    sender_name: &str,
    sender_role: Option<&str>,
    group_name: Option<&str>,
) -> ChatEvent {
    let (event_type, detail_type) = (
        EventType::Message,
        match message.session_type {
            SessionType::Group => "group",
            SessionType::Private => "private",
            SessionType::None => "private",
        },
    );
    ChatEvent {
        event_id: format!("message:{}", message.id),
        event_type,
        detail_type: detail_type.to_owned(),
        account_id: String::new(),
        session_type: message.session_type,
        session_id: match message.session_type {
            SessionType::Group => Some(message.session_id.clone()),
            _ => None,
        },
        sender_id: Some(message.sender_id.to_string()),
        content: message.content.clone(),
        timestamp: unix_seconds(message.created_at),
        raw: None,
        message_id: Some(message.id),
        message_seq: Some(message.message_seq),
        extras: EventExtras {
            nickname: Some(sender_name.to_owned()),
            card: None,
            role: sender_role.map(str::to_owned),
            group_name: group_name.map(str::to_owned),
            ..EventExtras::default()
        },
    }
}

pub async fn fanout_group_message(
    state: &AppState,
    message: &StoredMessage,
) -> Result<(), FanoutError> {
    let group_id: i64 = message
        .session_id
        .parse()
        .map_err(|_| FanoutError::BadSession(message.session_id.clone()))?;
    let members = state.cache.group_members(group_id).await?;
    let sender = members.iter().find(|m| m.account_id == message.sender_id);
    let sender_name = match sender {
        Some(member) => member.username.clone(),
        None => state.store.get_account(message.sender_id).await?.username,
    };
    let sender_role = sender.map(|m| m.role.as_str()).unwrap_or("member");
    let group_name = state.store.get_group(group_id).await.ok().map(|g| g.name);
    let base = build_message_event(
        message,
        &sender_name,
        Some(sender_role),
        group_name.as_deref(),
    );

    for member in members.iter() {
        let event = base
            .for_account(member.account_id.to_string())
            .with_session_id(Some(group_id.to_string()));
        let account = account_from_member(member);
        dispatch(state, &account, &event);
    }
    Ok(())
}

pub async fn fanout_private_message(
    state: &AppState,
    message: &StoredMessage,
    sender_id: i64,
    peer_id: i64,
) -> Result<(), FanoutError> {
    let accounts = state.cache.private_participants(sender_id, peer_id).await?;
    let sender = accounts
        .iter()
        .find(|a| a.id == sender_id)
        .ok_or_else(|| FanoutError::MissingAccount(sender_id))?;
    let base = build_message_event(message, &sender.username, None, None);

    let mut recipients = vec![sender_id, peer_id];
    recipients.sort_unstable();
    recipients.dedup();
    for receiver_id in recipients {
        let Some(receiver) = accounts.iter().find(|a| a.id == receiver_id) else {
            continue;
        };
        let peer = if receiver_id == sender_id {
            peer_id
        } else {
            sender_id
        };
        let event = base
            .for_account(receiver_id.to_string())
            .with_session_id(Some(peer.to_string()));
        dispatch(state, receiver, &event);
    }
    Ok(())
}

/// Push a membership-change notice to current group members. Callers invalidate
/// the group cache before calling this. `exclude` is used for group_decrease so
/// the leaver does not receive their own departure notice.
pub async fn fanout_group_notice(
    state: &AppState,
    group_id: i64,
    detail_type: &str,
    operator_id: i64,
    target_id: i64,
    sub_type: &str,
    exclude: &[i64],
) -> Result<(), FanoutError> {
    let members = state.cache.group_members(group_id).await?;
    let group = state.store.get_group(group_id).await?;
    let base = ChatEvent {
        event_id: new_event_id("notice"),
        event_type: EventType::Notice,
        detail_type: detail_type.to_owned(),
        account_id: String::new(),
        session_type: SessionType::Group,
        session_id: Some(group_id.to_string()),
        sender_id: Some(operator_id.to_string()),
        content: Vec::new(),
        timestamp: unix_seconds(Utc::now()),
        raw: None,
        message_id: None,
        message_seq: None,
        extras: EventExtras {
            group_name: Some(group.name),
            operator_id: Some(operator_id.to_string()),
            target_id: Some(target_id.to_string()),
            request_sub_type: Some(sub_type.to_owned()),
            ..EventExtras::default()
        },
    };

    let excluded: HashSet<i64> = exclude.iter().copied().collect();
    for member in members.iter() {
        if excluded.contains(&member.account_id) {
            continue;
        }
        let event = base.for_account(member.account_id.to_string());
        dispatch(state, &account_from_member(member), &event);
    }
    Ok(())
}

pub async fn fanout_friend_request(
    state: &AppState,
    request: &FriendRequestView,
) -> Result<(), FanoutError> {
    let from = state.store.get_account(request.from_id).await?;
    let to = state.store.get_account(request.to_id).await?;
    let event = ChatEvent {
        event_id: format!("friend-request:{}", request.id),
        event_type: EventType::Request,
        detail_type: "friend".to_owned(),
        account_id: to.id_string(),
        session_type: SessionType::None,
        session_id: None,
        sender_id: Some(from.id_string()),
        content: Vec::new(),
        timestamp: unix_seconds(request.created_at),
        raw: None,
        message_id: None,
        message_seq: None,
        extras: EventExtras {
            nickname: Some(from.username.clone()),
            request_flag: Some(request.id.to_string()),
            request_comment: Some(format!(
                "{} requested to add you as a friend",
                from.username
            )),
            ..EventExtras::default()
        },
    };
    dispatch(state, &to, &event);
    Ok(())
}

pub async fn fanout_group_join_request(
    state: &AppState,
    request: &GroupJoinRequestView,
) -> Result<(), FanoutError> {
    let group = state.store.get_group(request.group_id).await?;
    let requester = state.store.get_account(request.account_id).await?;
    let admins = state.store.list_group_admins(request.group_id).await?;
    let base = ChatEvent {
        event_id: format!("group-request:{}", request.id),
        event_type: EventType::Request,
        detail_type: "group".to_owned(),
        account_id: String::new(),
        session_type: SessionType::Group,
        session_id: Some(request.group_id.to_string()),
        sender_id: Some(requester.id_string()),
        content: Vec::new(),
        timestamp: unix_seconds(request.created_at),
        raw: None,
        message_id: None,
        message_seq: None,
        extras: EventExtras {
            nickname: Some(requester.username.clone()),
            group_name: Some(group.name.clone()),
            request_flag: Some(request.id.to_string()),
            request_comment: Some(format!(
                "{} requested to join {}",
                requester.username, group.name
            )),
            request_sub_type: Some("add".to_owned()),
            ..EventExtras::default()
        },
    };

    let mut seen = HashSet::new();
    for admin in admins {
        if !seen.insert(admin.account_id) {
            continue;
        }
        let event = base.for_account(admin.account_id.to_string());
        dispatch(state, &account_from_member(&admin), &event);
    }
    Ok(())
}

fn dispatch(state: &AppState, account: &Account, event: &ChatEvent) {
    match state.adapter.serialize_event(event, account) {
        Ok(payload) => {
            let report = state.registry.push(account.id, payload);
            if report.slow_closed > 0 {
                tracing::warn!(account_id = account.id, "fanout closed slow connection(s)");
            }
        }
        Err(err) => {
            tracing::error!(
                account_id = account.id,
                event_id = %event.event_id,
                error = %err,
                "adapter failed to serialize event"
            );
        }
    }
}

pub fn account_from_member(member: &GroupMemberView) -> Account {
    Account {
        id: member.account_id,
        username: member.username.clone(),
        labels: Vec::new(),
        rate_limit: 1,
        created_at: member.joined_at,
        disabled: false,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FanoutError {
    #[error("invalid session id: {0}")]
    BadSession(String),
    #[error("account {0} not found for fanout")]
    MissingAccount(i64),
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
}
