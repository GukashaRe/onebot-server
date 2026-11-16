use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};

static EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn unix_seconds(ts: DateTime<Utc>) -> f64 {
    ts.timestamp() as f64
}

pub fn new_event_id(prefix: &str) -> String {
    let n = EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}:{}:{n}", Utc::now().timestamp_millis())
}

pub fn json_id(id: i64) -> serde_json::Value {
    serde_json::Value::Number(id.into())
}

pub fn parse_id(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| {
            n.as_u64().and_then(|v| {
                if v <= i64::MAX as u64 {
                    Some(v as i64)
                } else {
                    None
                }
            })
        }),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

pub fn json_id_from_str(id: &str) -> serde_json::Value {
    id.parse::<i64>()
        .map(serde_json::Value::from)
        .unwrap_or_else(|_| serde_json::Value::String(id.to_owned()))
}
