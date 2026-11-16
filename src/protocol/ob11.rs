use serde_json::{Map, Value, json};

use crate::{
    error::AdapterError,
    model::{
        Account, ActionRequest, ActionResult, ChatEvent, EventExtras, EventType, InternalAction,
        MessageSegment, SessionType,
    },
    protocol::{
        ActionMapping, ProtocolAdapter,
        segments::{deserialize_segments, raw_message, serialize_segments},
    },
    util::{json_id, json_id_from_str, new_event_id, parse_id},
};

const IMPLEMENTED_ACTIONS: &[&str] = &[
    "send_private_msg",
    "send_group_msg",
    "send_msg",
    "get_login_info",
    "get_group_list",
    "get_group_member_list",
    "get_group_member_info",
    "get_friend_list",
    "get_group_msg_history",
    "get_friend_msg_history",
];

pub struct Ob11Adapter;

impl Ob11Adapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Ob11Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolAdapter for Ob11Adapter {
    fn deserialize_event(&self, raw: &str) -> Result<ChatEvent, AdapterError> {
        let value: Value = serde_json::from_str(raw)?;
        let object = value.as_object().ok_or_else(|| {
            AdapterError::InvalidPayload("event must be a JSON object".to_owned())
        })?;
        let post_type = required_str(object, "post_type")?;
        let time = object.get("time").and_then(Value::as_f64).unwrap_or(0.0);
        let self_id = object
            .get("self_id")
            .and_then(parse_id)
            .ok_or_else(|| AdapterError::InvalidPayload("missing self_id".to_owned()))?;
        let account_id = self_id.to_string();

        match post_type {
            "meta" => {
                let detail = object
                    .get("meta_event_type")
                    .and_then(Value::as_str)
                    .unwrap_or("meta")
                    .to_owned();
                Ok(ChatEvent {
                    event_id: new_event_id("meta"),
                    event_type: EventType::Meta,
                    detail_type: detail,
                    account_id,
                    session_type: SessionType::None,
                    session_id: None,
                    sender_id: None,
                    content: Vec::new(),
                    timestamp: time,
                    raw: Some(value.clone()),
                    message_id: None,
                    message_seq: None,
                    extras: EventExtras::default(),
                })
            }
            "message" => {
                let message_type = required_str(object, "message_type")?;
                let session_type = match message_type {
                    "group" => SessionType::Group,
                    "private" => SessionType::Private,
                    other => {
                        return Err(AdapterError::InvalidPayload(format!(
                            "unsupported message_type {other}"
                        )));
                    }
                };
                let sender_id = object
                    .get("user_id")
                    .and_then(parse_id)
                    .ok_or_else(|| AdapterError::InvalidPayload("missing user_id".to_owned()))?;
                let group_id = object.get("group_id").and_then(parse_id);
                let session_id = match session_type {
                    SessionType::Group => group_id.map(|id| id.to_string()).ok_or_else(|| {
                        AdapterError::InvalidPayload("missing group_id".to_owned())
                    })?,
                    SessionType::Private => sender_id.to_string(),
                    SessionType::None => unreachable!(),
                };
                let content = object
                    .get("message")
                    .map(deserialize_segments)
                    .transpose()?
                    .unwrap_or_default();
                let message_id = object.get("message_id").and_then(parse_id);
                let mut extras = EventExtras::default();
                if let Some(sender) = object.get("sender").and_then(Value::as_object) {
                    extras.nickname = sender
                        .get("nickname")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    extras.card = sender
                        .get("card")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    extras.role = sender
                        .get("role")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                Ok(ChatEvent {
                    event_id: message_id
                        .map(|id| format!("message:{id}"))
                        .unwrap_or_else(|| new_event_id("message")),
                    event_type: EventType::Message,
                    detail_type: message_type.to_owned(),
                    account_id,
                    session_type,
                    session_id: Some(session_id),
                    sender_id: Some(sender_id.to_string()),
                    content,
                    timestamp: time,
                    raw: Some(value.clone()),
                    message_id,
                    message_seq: object.get("message_seq").and_then(parse_id),
                    extras,
                })
            }
            "notice" => {
                let notice_type = required_str(object, "notice_type")?;
                let session_id = object
                    .get("group_id")
                    .and_then(parse_id)
                    .map(|id| id.to_string());
                let sender_id = object
                    .get("user_id")
                    .and_then(parse_id)
                    .map(|id| id.to_string());
                let operator_id = object
                    .get("operator_id")
                    .and_then(parse_id)
                    .map(|id| id.to_string());
                let sub_type = object
                    .get("sub_type")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                Ok(ChatEvent {
                    event_id: new_event_id("notice"),
                    event_type: EventType::Notice,
                    detail_type: notice_type.to_owned(),
                    account_id,
                    session_type: if session_id.is_some() {
                        SessionType::Group
                    } else {
                        SessionType::Private
                    },
                    session_id,
                    sender_id,
                    content: Vec::new(),
                    timestamp: time,
                    raw: Some(value.clone()),
                    message_id: None,
                    message_seq: None,
                    extras: EventExtras {
                        operator_id,
                        target_id: object
                            .get("target_id")
                            .and_then(parse_id)
                            .map(|id| id.to_string()),
                        request_sub_type: sub_type,
                        ..EventExtras::default()
                    },
                })
            }
            "request" => {
                let request_type = required_str(object, "request_type")?;
                let sender_id = object
                    .get("user_id")
                    .and_then(parse_id)
                    .map(|id| id.to_string());
                let group_id = object.get("group_id").and_then(parse_id);
                let session_id = group_id.map(|id| id.to_string());
                let sub_type = object
                    .get("sub_type")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                Ok(ChatEvent {
                    event_id: new_event_id("request"),
                    event_type: EventType::Request,
                    detail_type: request_type.to_owned(),
                    account_id,
                    session_type: if session_id.is_some() {
                        SessionType::Group
                    } else {
                        SessionType::None
                    },
                    session_id,
                    sender_id,
                    content: Vec::new(),
                    timestamp: time,
                    raw: Some(value.clone()),
                    message_id: None,
                    message_seq: None,
                    extras: EventExtras {
                        request_sub_type: sub_type,
                        request_flag: object
                            .get("flag")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        request_comment: object
                            .get("comment")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        ..EventExtras::default()
                    },
                })
            }
            other => Err(AdapterError::InvalidPayload(format!(
                "unsupported post_type {other}"
            ))),
        }
    }

    fn serialize_event(
        &self,
        event: &ChatEvent,
        self_account: &Account,
    ) -> Result<String, AdapterError> {
        let value = match event.event_type {
            EventType::Message => serialize_message_event(event, self_account)?,
            EventType::Notice => serialize_notice_event(event, self_account)?,
            EventType::Request => serialize_request_event(event, self_account)?,
            EventType::Meta => serialize_meta_event(event, self_account),
        };
        serde_json::to_string(&value).map_err(AdapterError::from)
    }

    fn deserialize_action(&self, raw: &str) -> Result<ActionRequest, AdapterError> {
        let value: Value = serde_json::from_str(raw)?;
        let object = value.as_object().ok_or_else(|| {
            AdapterError::InvalidPayload("action body must be an object".to_owned())
        })?;
        let action = object
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::InvalidParameter("action".to_owned()))?;
        let params = object.get("params").and_then(Value::as_object);

        let action = match action {
            "send_private_msg" => InternalAction::SendPrivateMsg {
                user_id: required_param_id(params, "user_id")?,
                message: required_param_message(params)?,
            },
            "send_group_msg" => InternalAction::SendGroupMsg {
                group_id: required_param_id(params, "group_id")?,
                message: required_param_message(params)?,
            },
            "send_msg" => {
                let message_type = params
                    .and_then(|p| p.get("message_type"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let user_id = optional_param_id(params, "user_id")?;
                let group_id = optional_param_id(params, "group_id")?;
                if message_type.is_none() && user_id.is_none() && group_id.is_none() {
                    return Err(AdapterError::InvalidParameter(
                        "send_msg requires message_type or a target id".to_owned(),
                    ));
                }
                InternalAction::SendMsg {
                    message_type,
                    user_id,
                    group_id,
                    message: required_param_message(params)?,
                }
            }
            "get_login_info" => InternalAction::GetLoginInfo,
            "get_group_list" => InternalAction::GetGroupList,
            "get_group_member_list" => InternalAction::GetGroupMemberList {
                group_id: required_param_id(params, "group_id")?,
            },
            "get_group_member_info" => InternalAction::GetGroupMemberInfo {
                group_id: required_param_id(params, "group_id")?,
                user_id: required_param_id(params, "user_id")?,
            },
            "get_friend_list" => InternalAction::GetFriendList,
            "get_group_msg_history" => InternalAction::GetGroupMsgHistory {
                group_id: required_param_id(params, "group_id")?,
                before_seq: optional_cursor(params, "before_seq")?,
                after_seq: optional_cursor(params, "after_seq")?,
                limit: optional_limit(params)?,
            },
            "get_friend_msg_history" => InternalAction::GetFriendMsgHistory {
                user_id: required_param_id(params, "user_id")?,
                before_seq: optional_cursor(params, "before_seq")?,
                after_seq: optional_cursor(params, "after_seq")?,
                limit: optional_limit(params)?,
            },
            other => InternalAction::Unsupported {
                name: other.to_owned(),
            },
        };

        Ok(ActionRequest { action })
    }

    fn serialize_action_response(&self, resp: &ActionResult) -> Result<String, AdapterError> {
        let retcode = if resp.success {
            0
        } else {
            resp.error_code.unwrap_or(-1)
        };
        let value = json!({
            "status": if resp.success { "ok" } else { "failed" },
            "retcode": retcode,
            "data": if resp.success { resp.data.clone() } else { Value::Null },
        });
        Ok(value.to_string())
    }

    fn action_mapping(&self) -> ActionMapping {
        ActionMapping {
            protocol: "OneBot",
            version: "11",
            implemented_actions: IMPLEMENTED_ACTIONS.to_vec(),
        }
    }
}

fn serialize_message_event(
    event: &ChatEvent,
    self_account: &Account,
) -> Result<Value, AdapterError> {
    let message_id = event.message_id.ok_or_else(|| {
        AdapterError::InvalidPayload("message event missing message_id".to_owned())
    })?;
    let sender_id = event
        .sender_id
        .as_deref()
        .and_then(|id| id.parse::<i64>().ok())
        .ok_or_else(|| {
            AdapterError::InvalidPayload("message event missing sender_id".to_owned())
        })?;
    let mut object = Map::new();
    object.insert("post_type".to_owned(), json!("message"));
    object.insert("message_type".to_owned(), json!(event.detail_type));
    object.insert(
        "sub_type".to_owned(),
        json!(match event.session_type {
            SessionType::Group => "normal",
            _ => "friend",
        }),
    );
    object.insert("time".to_owned(), json!(event.timestamp.floor() as i64));
    object.insert("self_id".to_owned(), json_id(self_account.id));
    object.insert("message_id".to_owned(), json!(message_id));
    object.insert("user_id".to_owned(), json!(sender_id));
    if let Some(seq) = event.message_seq {
        // Prototype extension used by history cursor pagination.
        object.insert("message_seq".to_owned(), json!(seq));
    }
    if event.session_type == SessionType::Group {
        let group_id = event
            .session_id
            .as_deref()
            .and_then(|id| id.parse::<i64>().ok())
            .ok_or_else(|| {
                AdapterError::InvalidPayload("group event missing session_id".to_owned())
            })?;
        object.insert("group_id".to_owned(), json!(group_id));
    } else if event.session_type == SessionType::Private {
        // Prototype extension: OB11 private events do not identify the peer for
        // the sender's echo (user_id is the sender itself). Browser clients need
        // this to route outgoing messages to the right conversation.
        if let Some(peer_id) = &event.session_id {
            object.insert("peer_id".to_owned(), json_id_from_str(peer_id));
        }
    }
    let segments = serialize_segments(&event.content);
    object.insert("raw_message".to_owned(), json!(raw_message(&event.content)));
    object.insert("message".to_owned(), Value::Array(segments));
    object.insert("font".to_owned(), json!(0));
    object.insert(
        "sender".to_owned(),
        json!({
            "user_id": sender_id,
            "nickname": event.extras.nickname.clone().unwrap_or_default(),
            "card": event.extras.card.clone().unwrap_or_default(),
            "role": event.extras.role.clone().unwrap_or_else(|| "member".to_owned()),
        }),
    );
    Ok(Value::Object(object))
}

fn serialize_notice_event(
    event: &ChatEvent,
    self_account: &Account,
) -> Result<Value, AdapterError> {
    let mut object = Map::new();
    object.insert("post_type".to_owned(), json!("notice"));
    object.insert("notice_type".to_owned(), json!(event.detail_type));
    if let Some(sub_type) = &event.extras.request_sub_type {
        object.insert("sub_type".to_owned(), json!(sub_type));
    }
    object.insert("time".to_owned(), json!(event.timestamp.floor() as i64));
    object.insert("self_id".to_owned(), json_id(self_account.id));
    if let Some(group_id) = &event.session_id {
        object.insert("group_id".to_owned(), json_id_from_str(group_id));
    }
    if let Some(operator_id) = &event.extras.operator_id {
        object.insert("operator_id".to_owned(), json_id_from_str(operator_id));
    }
    if let Some(target_id) = &event.extras.target_id {
        object.insert("user_id".to_owned(), json_id_from_str(target_id));
    } else if let Some(sender_id) = &event.sender_id {
        object.insert("user_id".to_owned(), json_id_from_str(sender_id));
    }
    Ok(Value::Object(object))
}

fn serialize_request_event(
    event: &ChatEvent,
    self_account: &Account,
) -> Result<Value, AdapterError> {
    let mut object = Map::new();
    object.insert("post_type".to_owned(), json!("request"));
    object.insert("request_type".to_owned(), json!(event.detail_type));
    object.insert("time".to_owned(), json!(event.timestamp.floor() as i64));
    object.insert("self_id".to_owned(), json_id(self_account.id));
    if event.detail_type == "group" {
        object.insert(
            "sub_type".to_owned(),
            json!(
                event
                    .extras
                    .request_sub_type
                    .clone()
                    .unwrap_or_else(|| "add".to_owned())
            ),
        );
        let group_id = event
            .session_id
            .as_deref()
            .and_then(|id| id.parse::<i64>().ok())
            .ok_or_else(|| {
                AdapterError::InvalidPayload("group request missing session_id".to_owned())
            })?;
        object.insert("group_id".to_owned(), json!(group_id));
    }
    if let Some(sender_id) = &event.sender_id {
        object.insert("user_id".to_owned(), json_id_from_str(sender_id));
    }
    object.insert(
        "comment".to_owned(),
        json!(event.extras.request_comment.clone().unwrap_or_default()),
    );
    object.insert(
        "flag".to_owned(),
        json!(event.extras.request_flag.clone().unwrap_or_default()),
    );
    Ok(Value::Object(object))
}

fn serialize_meta_event(event: &ChatEvent, self_account: &Account) -> Value {
    json!({
        "post_type": "meta_event",
        "meta_event_type": event.detail_type,
        "time": event.timestamp.floor() as i64,
        "self_id": self_account.id,
    })
}

fn required_str<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, AdapterError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::InvalidPayload(format!("missing {key}")))
}

fn required_param_id(params: Option<&Map<String, Value>>, key: &str) -> Result<i64, AdapterError> {
    let value = params
        .and_then(|p| p.get(key))
        .ok_or_else(|| AdapterError::InvalidParameter(key.to_owned()))?;
    parse_id(value).ok_or_else(|| AdapterError::InvalidParameter(key.to_owned()))
}

fn optional_param_id(
    params: Option<&Map<String, Value>>,
    key: &str,
) -> Result<Option<i64>, AdapterError> {
    match params.and_then(|p| p.get(key)) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => parse_id(value)
            .map(Some)
            .ok_or_else(|| AdapterError::InvalidParameter(key.to_owned())),
    }
}

fn required_param_message(
    params: Option<&Map<String, Value>>,
) -> Result<Vec<MessageSegment>, AdapterError> {
    let value = params
        .and_then(|p| p.get("message"))
        .ok_or_else(|| AdapterError::InvalidParameter("message".to_owned()))?;
    deserialize_segments(value)
}

fn optional_cursor(
    params: Option<&Map<String, Value>>,
    key: &str,
) -> Result<Option<i64>, AdapterError> {
    match params.and_then(|p| p.get(key)) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let parsed =
                parse_id(value).ok_or_else(|| AdapterError::InvalidParameter(key.to_owned()))?;
            if parsed < 0 {
                return Err(AdapterError::InvalidParameter(format!(
                    "{key} must be >= 0"
                )));
            }
            Ok(Some(parsed))
        }
    }
}

fn optional_limit(params: Option<&Map<String, Value>>) -> Result<Option<i64>, AdapterError> {
    match params.and_then(|p| p.get("limit")) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let parsed = parse_id(value)
                .ok_or_else(|| AdapterError::InvalidParameter("limit".to_owned()))?;
            if parsed <= 0 {
                return Err(AdapterError::InvalidParameter(
                    "limit must be greater than zero".to_owned(),
                ));
            }
            Ok(Some(parsed))
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::model::{EventExtras, EventType, MessageSegment, SessionType};

    fn account(id: i64) -> Account {
        Account {
            id,
            username: format!("account_{id}"),
            labels: Vec::new(),
            rate_limit: 20,
            created_at: Utc::now(),
            disabled: false,
        }
    }

    #[test]
    fn private_action_is_parsed_into_internal_action() {
        let adapter = Ob11Adapter::new();
        let request = adapter
            .deserialize_action(
                r#"{"action":"send_private_msg","params":{"user_id":"2","message":[{"type":"text","data":{"text":"hi"}}]}}"#,
            )
            .unwrap();
        match request.action {
            InternalAction::SendPrivateMsg { user_id, message } => {
                assert_eq!(user_id, 2);
                assert_eq!(message, vec![MessageSegment::text("hi")]);
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn cq_string_message_is_rejected_as_todo() {
        let adapter = Ob11Adapter::new();
        let error = adapter
            .deserialize_action(
                r#"{"action":"send_private_msg","params":{"user_id":2,"message":"[CQ:face,id=1]"}}"#,
            )
            .unwrap_err();
        assert!(error.to_string().contains("CQ string"));
    }

    #[test]
    fn unsupported_action_has_standard_failure_envelope() {
        let adapter = Ob11Adapter::new();
        let request = adapter
            .deserialize_action(r#"{"action":"set_group_kick","params":{}}"#)
            .unwrap();
        assert!(matches!(request.action, InternalAction::Unsupported { .. }));

        let body = adapter
            .serialize_action_response(&ActionResult::fail(1404))
            .unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["status"], "failed");
        assert_eq!(value["retcode"], 1404);
        assert_eq!(value["data"], Value::Null);
        assert_eq!(value.as_object().unwrap().len(), 3);
    }

    #[test]
    fn serialization_uses_the_receiving_account_as_self_id() {
        let adapter = Ob11Adapter::new();
        let event = ChatEvent {
            event_id: "message:1".to_owned(),
            event_type: EventType::Message,
            detail_type: "group".to_owned(),
            account_id: "1".to_owned(),
            session_type: SessionType::Group,
            session_id: Some("10".to_owned()),
            sender_id: Some("1".to_owned()),
            content: vec![MessageSegment::text("hello")],
            timestamp: 1_700_000_000.0,
            raw: None,
            message_id: Some(42),
            message_seq: Some(3),
            extras: EventExtras {
                nickname: Some("alice".to_owned()),
                role: Some("owner".to_owned()),
                ..EventExtras::default()
            },
        };
        let body = adapter.serialize_event(&event, &account(2)).unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["self_id"], 2);
        assert_eq!(value["message_type"], "group");
        assert_eq!(value["message"][0]["type"], "text");
        assert_eq!(value["message"][0]["data"]["text"], "hello");
        assert_eq!(value["message_seq"], 3);
    }

    #[test]
    fn private_serialization_exposes_peer_id_for_client_routing() {
        let adapter = Ob11Adapter::new();
        let event = ChatEvent {
            event_id: "message:2".to_owned(),
            event_type: EventType::Message,
            detail_type: "private".to_owned(),
            account_id: "1".to_owned(),
            session_type: SessionType::Private,
            session_id: Some("2".to_owned()),
            sender_id: Some("1".to_owned()),
            content: vec![MessageSegment::text("hello")],
            timestamp: 1_700_000_000.0,
            raw: None,
            message_id: Some(43),
            message_seq: Some(1),
            extras: EventExtras::default(),
        };
        let body = adapter.serialize_event(&event, &account(1)).unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["peer_id"], 2);
        assert_eq!(value["self_id"], 1);
        assert_eq!(value["user_id"], 1);
    }

    #[test]
    fn deserialize_event_maps_ob11_fields_to_neutral_event() {
        let adapter = Ob11Adapter::new();
        let event = adapter
            .deserialize_event(
                r#"{
                    "post_type":"message","message_type":"private","sub_type":"friend",
                    "time":1700000000,"self_id":2,"message_id":9,"user_id":1,
                    "message":[{"type":"text","data":{"text":"hi"}}],
                    "sender":{"user_id":1,"nickname":"alice"}
                }"#,
            )
            .unwrap();
        assert_eq!(event.event_type, EventType::Message);
        assert_eq!(event.detail_type, "private");
        assert_eq!(event.account_id, "2");
        assert_eq!(event.session_type, SessionType::Private);
        assert_eq!(event.session_id.as_deref(), Some("1"));
        assert_eq!(event.sender_id.as_deref(), Some("1"));
        assert_eq!(event.message_id, Some(9));
    }
}
