use crate::{
    error::AdapterError,
    model::{Account, ActionRequest, ActionResult, ChatEvent},
};

pub mod ob11;
pub mod ob12;
pub mod segments;

#[derive(Clone, Debug)]
pub struct ActionMapping {
    pub protocol: &'static str,
    pub version: &'static str,
    pub implemented_actions: Vec<&'static str>,
}

/// All protocol-specific serialization is behind this boundary. Storage and
/// fanout only ever see `ChatEvent` / `ActionRequest` / `ActionResult`.
pub trait ProtocolAdapter: Send + Sync {
    fn deserialize_event(&self, raw: &str) -> Result<ChatEvent, AdapterError>;

    fn serialize_event(
        &self,
        event: &ChatEvent,
        self_account: &Account,
    ) -> Result<String, AdapterError>;

    fn deserialize_action(&self, raw: &str) -> Result<ActionRequest, AdapterError>;

    fn serialize_action_response(&self, resp: &ActionResult) -> Result<String, AdapterError>;

    fn action_mapping(&self) -> ActionMapping;

    /// Events that must be sent before any other event once a WS is established.
    /// OB11 has none. The OB12 adapter must override this with a meta.connect
    /// frame after implementing the protocol.
    fn initial_events(&self, self_account: &Account) -> Result<Vec<String>, AdapterError> {
        let _ = self_account;
        Ok(Vec::new())
    }
}
