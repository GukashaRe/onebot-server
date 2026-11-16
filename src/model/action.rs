use serde_json::Value;

use crate::model::MessageSegment;

#[derive(Clone, Debug)]
pub enum InternalAction {
    SendPrivateMsg {
        user_id: i64,
        message: Vec<MessageSegment>,
    },
    SendGroupMsg {
        group_id: i64,
        message: Vec<MessageSegment>,
    },
    SendMsg {
        message_type: Option<String>,
        user_id: Option<i64>,
        group_id: Option<i64>,
        message: Vec<MessageSegment>,
    },
    GetLoginInfo,
    GetGroupList,
    GetGroupMemberList {
        group_id: i64,
    },
    GetGroupMemberInfo {
        group_id: i64,
        user_id: i64,
    },
    GetFriendList,
    GetGroupMsgHistory {
        group_id: i64,
        before_seq: Option<i64>,
        after_seq: Option<i64>,
        limit: Option<i64>,
    },
    GetFriendMsgHistory {
        user_id: i64,
        before_seq: Option<i64>,
        after_seq: Option<i64>,
        limit: Option<i64>,
    },
    SetGroupKick {
        group_id: i64,
        user_id: i64,
        reject_add_request: Option<bool>,
    },
    SetGroupBan {
        group_id: i64,
        user_id: i64,
        duration_seconds: i64,
    },
    SetGroupAdmin {
        group_id: i64,
        user_id: i64,
        enable: bool,
    },
    SetGroupCard {
        group_id: i64,
        user_id: i64,
        card: String,
    },
    /// A syntactically valid, but unsupported OB11 action. It must still yield a
    /// standard OB11 failure response (retcode 1404), never HTTP 500.
    Unsupported {
        name: String,
    },
}

#[derive(Clone, Debug)]
pub struct ActionRequest {
    pub action: InternalAction,
}

#[derive(Clone, Debug)]
pub struct ActionResult {
    pub success: bool,
    pub data: Value,
    pub error_code: Option<i32>,
}

impl ActionResult {
    pub fn ok(data: Value) -> Self {
        Self {
            success: true,
            data,
            error_code: None,
        }
    }

    pub fn fail(error_code: i32) -> Self {
        Self {
            success: false,
            data: Value::Null,
            error_code: Some(error_code),
        }
    }
}
