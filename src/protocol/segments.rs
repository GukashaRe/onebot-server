//! OneBot 11 message-segment wire conversion.
//!
//! Storage and fanout never see OB11's `{"type","data"}` shape; they only see
//! `MessageSegment`. CQ string encoding/decoding is deliberately TODO.

use serde_json::{Map, Value, json};

use crate::{error::AdapterError, model::MessageSegment};

/// Parse an OB11 segment array. Strings (CQ codes) are rejected for the prototype.
pub fn deserialize_segments(value: &Value) -> Result<Vec<MessageSegment>, AdapterError> {
    let array = value.as_array().ok_or_else(|| {
        AdapterError::InvalidPayload(
            "message must be a segment array; CQ string decoding is TODO".to_owned(),
        )
    })?;
    let mut segments = Vec::with_capacity(array.len());
    for item in array {
        let object = item.as_object().ok_or_else(|| {
            AdapterError::InvalidPayload("message segment must be an object".to_owned())
        })?;
        let segment_type = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::InvalidPayload("segment missing type".to_owned()))?;
        if segment_type.is_empty() {
            return Err(AdapterError::InvalidPayload(
                "segment type is empty".to_owned(),
            ));
        }
        let data = object
            .get("data")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        segments.push(MessageSegment {
            segment_type: segment_type.to_owned(),
            data,
        });
    }
    Ok(segments)
}

/// Render neutral segments into the OB11 message array.
pub fn serialize_segments(segments: &[MessageSegment]) -> Vec<Value> {
    segments
        .iter()
        .map(|segment| {
            let mut object = Map::new();
            object.insert("type".to_owned(), json!(segment.segment_type));
            object.insert("data".to_owned(), Value::Object(segment.data.clone()));
            Value::Object(object)
        })
        .collect()
}

/// Approximate OB11 raw_message. Text segments are concatenated; other segments
/// use `[type]` placeholders because full CQ encoding is TODO.
pub fn raw_message(segments: &[MessageSegment]) -> String {
    let mut out = String::new();
    for segment in segments {
        if let Some(text) = segment.plain_text() {
            out.push_str(text);
        } else {
            out.push('[');
            out.push_str(&segment.segment_type);
            out.push(']');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn round_trips_segment_arrays() {
        let value = json!([
            {"type":"text","data":{"text":"hello"}},
            {"type":"image","data":{"file":"a.png"}}
        ]);
        let segments = deserialize_segments(&value).unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].plain_text(), Some("hello"));
        assert_eq!(raw_message(&segments), "hello[image]");
        let rendered = serialize_segments(&segments);
        assert_eq!(Value::Array(rendered), value);
    }

    #[test]
    fn cq_string_is_rejected() {
        let error = deserialize_segments(&json!("[CQ:face,id=1]")).unwrap_err();
        assert!(error.to_string().contains("CQ string"));
    }
}
