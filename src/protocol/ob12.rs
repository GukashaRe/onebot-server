use crate::{
    error::{ActionError, AdapterError, RETCODE_NOT_FOUND},
    model::{Account, ActionRequest, ActionResult, ChatEvent},
    protocol::{ActionMapping, ProtocolAdapter},
};

/// OneBot 12 placeholder. The rest of the server is intentionally independent of
/// OB11 wire fields, so implementing OB12 should require only this adapter plus
/// the WS handshake's `meta.connect` first-frame behavior.
///
/// TODO(OB12):
/// - deserialize_event: map type/detail_type/sub_type + self.user_id to ChatEvent.
/// - serialize_event: ChatEvent -> OB12 event; fill self.user_id from Account.
/// - deserialize_action / serialize_action_response: OB12 action envelope and
///   `retcode` semantics.
/// - initial_events: emit `{"type":"meta","detail_type":"connect",...}` as the
///   very first frame after authentication, with protocol version and self id.
/// - map OB12 action names to the same InternalAction variants.
#[allow(dead_code)]
pub struct Ob12Adapter;

impl Ob12Adapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Ob12Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolAdapter for Ob12Adapter {
    fn deserialize_event(&self, _raw: &str) -> Result<ChatEvent, AdapterError> {
        Err(AdapterError::UnsupportedProtocol(
            "OneBot 12 adapter is a TODO; use OB11 for the prototype".to_owned(),
        ))
    }

    fn serialize_event(
        &self,
        _event: &ChatEvent,
        _self_account: &Account,
    ) -> Result<String, AdapterError> {
        Err(AdapterError::UnsupportedProtocol(
            "OneBot 12 adapter is a TODO; use OB11 for the prototype".to_owned(),
        ))
    }

    fn deserialize_action(&self, _raw: &str) -> Result<ActionRequest, AdapterError> {
        Err(AdapterError::UnsupportedProtocol(
            "OneBot 12 adapter is a TODO; use OB11 for the prototype".to_owned(),
        ))
    }

    fn serialize_action_response(&self, _resp: &ActionResult) -> Result<String, AdapterError> {
        Err(AdapterError::UnsupportedProtocol(
            "OneBot 12 adapter is a TODO; use OB11 for the prototype".to_owned(),
        ))
    }

    fn action_mapping(&self) -> ActionMapping {
        ActionMapping {
            protocol: "OneBot",
            version: "12",
            implemented_actions: Vec::new(),
        }
    }

    fn initial_events(&self, _self_account: &Account) -> Result<Vec<String>, AdapterError> {
        // TODO(OB12): meta.connect handshake.
        Err(AdapterError::UnsupportedProtocol(
            "OneBot 12 meta.connect handshake is not implemented".to_owned(),
        ))
    }
}

// Keep the intended action error shape visible for the eventual implementation.
#[allow(dead_code)]
fn _ob12_unsupported_action(action: &str) -> ActionError {
    ActionError::new(
        RETCODE_NOT_FOUND,
        format!("action '{action}' is unsupported"),
    )
}
