use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::model::SessionType;

/// Internal message segment. This is intentionally *not* the OB11 wire format:
/// storage serializes `{"segment_type":"text","data":{...}}`; the OB11 adapter
/// renders `{"type":"text","data":{...}}`. CQ string conversion is TODO.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageSegment {
    pub segment_type: String,
    #[serde(default)]
    pub data: Map<String, Value>,
}

impl MessageSegment {
    pub fn text(text: impl Into<String>) -> Self {
        let mut data = Map::new();
        data.insert("text".to_owned(), Value::String(text.into()));
        Self {
            segment_type: "text".to_owned(),
            data,
        }
    }

    pub fn plain_text(&self) -> Option<&str> {
        if self.segment_type == "text" {
            self.data.get("text").and_then(Value::as_str)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub session_type: SessionType,
    pub session_id: String,
    pub sender_id: i64,
    pub message_seq: i64,
    pub content: Vec<MessageSegment>,
    pub created_at: DateTime<Utc>,
    pub recalled: bool,
}

#[derive(Clone, Debug)]
pub struct GroupMemberView {
    pub group_id: i64,
    pub account_id: i64,
    pub username: String,
    pub role: String,
    pub card: String,
    pub joined_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct GroupView {
    pub id: i64,
    pub name: String,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub member_count: i64,
    pub self_role: String,
}

#[derive(Clone, Debug)]
pub struct FriendView {
    pub account_id: i64,
    pub username: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct FriendRequestView {
    pub id: i64,
    pub from_id: i64,
    pub to_id: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct GroupJoinRequestView {
    pub id: i64,
    pub group_id: i64,
    pub account_id: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct IncomingFriendRequest {
    pub request_id: i64,
    pub from_id: i64,
    pub from_username: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ActionableGroupJoinRequest {
    pub request_id: i64,
    pub group_id: i64,
    pub group_name: String,
    pub account_id: i64,
    pub account_username: String,
    pub created_at: DateTime<Utc>,
}

/// Private conversations are keyed by the numeric ordered pair (min:max); see
/// README. This is storage-neutral and deterministic for both participants.
pub fn private_session_id(a: i64, b: i64) -> String {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    format!("{lo}:{hi}")
}

#[cfg(test)]
mod tests {
    use super::private_session_id;

    #[test]
    fn private_session_id_is_order_independent() {
        assert_eq!(private_session_id(2, 1), "1:2");
        assert_eq!(private_session_id(1, 2), "1:2");
        assert_eq!(private_session_id(10, 2), "2:10");
        assert_eq!(private_session_id(2, 10), "2:10");
    }
}
