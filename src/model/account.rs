use chrono::{DateTime, Utc};

/// An account is just an account. There is deliberately no human/bot distinction
/// anywhere in the server.
#[derive(Clone, Debug)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub labels: Vec<String>,
    pub rate_limit: i32,
    pub created_at: DateTime<Utc>,
    pub disabled: bool,
}

impl Account {
    pub fn id_string(&self) -> String {
        self.id.to_string()
    }
}
