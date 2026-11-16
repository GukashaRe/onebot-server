use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::model::MessageSegment;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventType {
    Meta,
    Message,
    Notice,
    Request,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Meta => "meta",
            Self::Message => "message",
            Self::Notice => "notice",
            Self::Request => "request",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionType {
    Group,
    Private,
    #[default]
    None,
}

impl SessionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Private => "private",
            Self::None => "none",
        }
    }
}

/// Extra, protocol-neutral facts that adapters may need when rendering an
/// event. Values are already normalized (for example, `role` is
/// owner/admin/member, not an adapter-specific enum).
#[derive(Clone, Debug, Default)]
pub struct EventExtras {
    pub nickname: Option<String>,
    pub card: Option<String>,
    pub role: Option<String>,
    pub group_name: Option<String>,
    pub operator_id: Option<String>,
    pub target_id: Option<String>,
    pub request_flag: Option<String>,
    pub request_comment: Option<String>,
    /// Used for group requests ("add") and notices ("approve"/"leave"/"kick").
    pub request_sub_type: Option<String>,
}

/// Protocol-neutral event. Storage and fanout manipulate only this structure;
/// field names such as post_type/self_id exist exclusively in adapters.
#[derive(Clone, Debug)]
pub struct ChatEvent {
    pub event_id: String,
    pub event_type: EventType,
    pub detail_type: String,
    pub account_id: String,
    pub session_type: SessionType,
    pub session_id: Option<String>,
    pub sender_id: Option<String>,
    pub content: Vec<MessageSegment>,
    pub timestamp: f64,
    pub raw: Option<Value>,
    pub message_id: Option<i64>,
    pub message_seq: Option<i64>,
    pub extras: EventExtras,
}

impl ChatEvent {
    pub fn for_account(&self, account_id: impl Into<String>) -> Self {
        let mut cloned = self.clone();
        cloned.account_id = account_id.into();
        cloned
    }

    pub fn with_session_id(&self, session_id: Option<String>) -> Self {
        let mut cloned = self.clone();
        cloned.session_id = session_id;
        cloned
    }

    pub fn raw_map(&self) -> Map<String, Value> {
        match &self.raw {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        }
    }
}
