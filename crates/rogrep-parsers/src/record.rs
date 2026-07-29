//! Defensive JSON record access. Provider formats drift; every helper
//! degrades to "stringify" rather than dropping data.

use serde_json::{Map, Value};

/// One parsed JSONL record.
pub struct RawRecord {
    pub obj: Map<String, Value>,
}

impl RawRecord {
    pub fn parse(bytes: &[u8]) -> Option<RawRecord> {
        match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(obj)) => Some(RawRecord { obj }),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.obj.get(key)
    }

    pub fn str_field(&self, key: &str) -> String {
        string_value(self.obj.get(key))
    }

    pub fn record_type(&self) -> String {
        self.str_field("type")
    }

    pub fn object(&self, key: &str) -> Option<&Map<String, Value>> {
        self.obj.get(key).and_then(Value::as_object)
    }

    pub fn bool_field(&self, key: &str) -> Option<bool> {
        self.obj.get(key).and_then(Value::as_bool)
    }

    /// Record-level timestamp in unix millis, probing common field names.
    pub fn timestamp_millis(&self) -> Option<i64> {
        for key in ["timestamp", "created_at", "createdAt", "time", "ts"] {
            if let Some(v) = self.obj.get(key) {
                match v {
                    Value::String(s) => {
                        if let Some(ms) = rogrep_model::parse_timestamp(s) {
                            return Some(ms);
                        }
                    }
                    Value::Number(n) => {
                        if let Some(f) = n.as_f64() {
                            if let Some(ms) = rogrep_model::millis_from_number(f) {
                                return Some(ms);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }
}

/// Loose string coercion (agentpm stringValue): strings pass through
/// trimmed, numbers/bools stringify, everything else is "".
pub fn string_value(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

pub fn int_value(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

pub fn u64_value(v: Option<&Value>) -> u64 {
    int_value(v).map(|n| n.max(0) as u64).unwrap_or(0)
}

pub fn compact_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// Extract human text from an arbitrary content value: strings pass
/// through; arrays join their items; objects probe common text-bearing keys.
pub fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                let text = content_text(item);
                if !text.is_empty() {
                    parts.push(text);
                } else if item.is_object() {
                    let c = compact_json(item);
                    if !c.is_empty() && c != "{}" {
                        parts.push(c);
                    }
                }
            }
            parts.join("\n")
        }
        Value::Object(obj) => {
            for key in ["text", "content", "message", "summary", "result", "output"] {
                if let Some(inner) = obj.get(key) {
                    let text = content_text(inner);
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

/// Recursive best-effort text extraction from a whole record (generic
/// provider fallback).
pub fn extract_text(obj: &Map<String, Value>) -> String {
    for key in ["text", "content", "message", "payload", "item", "summary", "arguments", "prompt", "display"] {
        if let Some(v) = obj.get(key) {
            match v {
                Value::String(s) => {
                    let s = s.trim();
                    if !s.is_empty() {
                        return s.to_string();
                    }
                }
                Value::Object(inner) => {
                    let t = extract_text(inner);
                    if !t.is_empty() {
                        return t;
                    }
                }
                Value::Array(_) => {
                    let t = content_text(v);
                    if !t.is_empty() {
                        return t;
                    }
                }
                _ => {}
            }
        }
    }
    String::new()
}

/// Best-effort role extraction: message.role → payload.role → role/speaker →
/// type-based hints.
pub fn extract_role(obj: &Map<String, Value>) -> String {
    for path in [&["message", "role"][..], &["payload", "role"][..], &["role"][..], &["speaker"][..]] {
        let mut cur: Option<&Value> = None;
        let mut node: &Map<String, Value> = obj;
        for (i, key) in path.iter().enumerate() {
            if i + 1 == path.len() {
                cur = node.get(*key);
            } else if let Some(next) = node.get(*key).and_then(Value::as_object) {
                node = next;
            } else {
                cur = None;
                break;
            }
        }
        let role = string_value(cur).to_lowercase();
        if !role.is_empty() {
            return normalize_role(&role);
        }
    }
    let t = string_value(obj.get("type")).to_lowercase();
    if ["user", "assistant", "system", "tool", "human", "ai"].contains(&t.as_str()) {
        return normalize_role(&t);
    }
    String::new()
}

pub fn normalize_role(role: &str) -> String {
    match role.trim().to_lowercase().as_str() {
        "user" | "human" => "user",
        "assistant" | "ai" | "model" => "assistant",
        "system" | "developer" => "system",
        "tool" | "function" => "tool",
        other => return other.to_string(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(v: Value) -> RawRecord {
        RawRecord {
            obj: v.as_object().unwrap().clone(),
        }
    }

    #[test]
    fn timestamp_forms() {
        assert!(rec(json!({"timestamp": "2026-07-29T14:00:00Z"})).timestamp_millis().is_some());
        assert_eq!(
            rec(json!({"ts": 1753000000})).timestamp_millis(),
            Some(1753000000000)
        );
        assert_eq!(rec(json!({"x": 1})).timestamp_millis(), None);
    }

    #[test]
    fn content_text_shapes() {
        assert_eq!(content_text(&json!("hi")), "hi");
        assert_eq!(
            content_text(&json!([{"type":"text","text":"a"},{"type":"text","text":"b"}])),
            "a\nb"
        );
        assert_eq!(content_text(&json!({"content": [{"text": "x"}]})), "x");
    }

    #[test]
    fn role_extraction() {
        assert_eq!(extract_role(&rec(json!({"message":{"role":"user"}})).obj), "user");
        assert_eq!(extract_role(&rec(json!({"payload":{"role":"developer"}})).obj), "system");
        assert_eq!(extract_role(&rec(json!({"type":"human"})).obj), "user");
    }
}
