//! Special-turn detection shared across providers: task notifications,
//! scheduled prompts, compact boundaries. These are the user-shaped harness
//! echoes that must NOT open exchanges.

use rogrep_model::{Role, SpecialTurn, Turn};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Extract `<tag>body</tag>` when the whole (trimmed) text is exactly one
/// such block. Case-insensitive tags.
pub fn standalone_tagged_body(text: &str, tag: &str) -> Option<String> {
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if !lower.starts_with(&open) {
        return None;
    }
    let body_start = open.len();
    let end = lower[body_start..].find(&close)?;
    let remainder = &trimmed[body_start + end + close.len()..];
    if !remainder.trim().is_empty() {
        return None;
    }
    Some(trimmed[body_start..body_start + end].to_string())
}

/// Pull `<tag>value</tag>` fields out of a body.
pub fn tagged_fields(body: &str, tags: &[&str]) -> BTreeMap<String, String> {
    let lower = body.to_lowercase();
    let mut out = BTreeMap::new();
    for tag in tags {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if let Some(start) = lower.find(&open) {
            let body_start = start + open.len();
            if let Some(end) = lower[body_start..].find(&close) {
                let value = body[body_start..body_start + end].trim().to_string();
                if !value.is_empty() {
                    out.insert(tag.to_string(), value);
                }
            }
        }
    }
    out
}

fn short_hash(seed: &str) -> String {
    let sum = Sha256::digest(seed.as_bytes());
    sum.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn trim_text(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Detect a `<task-notification>` body and build the special turn.
pub fn task_notification_special(text: &str, queued: bool) -> Option<SpecialTurn> {
    let body = standalone_tagged_body(text, "task-notification")?;
    let fields = tagged_fields(
        &body,
        &["task-id", "tool-use-id", "output-file", "status", "summary", "result", "usage"],
    );
    if fields.is_empty() {
        return None;
    }
    let summary = fields.get("summary").cloned().unwrap_or_default();
    let status = fields.get("status").cloned();
    let seed = fields
        .get("task-id")
        .or_else(|| fields.get("tool-use-id"))
        .cloned()
        .unwrap_or_else(|| if summary.is_empty() { body.clone() } else { summary.clone() });
    let prefix = if queued { "task-notification-queued" } else { "task-notification" };
    Some(SpecialTurn::TaskNotification {
        queued,
        status,
        summary,
        signature: format!("{prefix}:{}", short_hash(&seed)),
    })
}

/// Detect a scheduled prompt: queued (queue-operation enqueue) or delivered
/// (meta user prompt carrying a `<scheduled-task …>` block).
pub fn scheduled_prompt_special(text: &str, queued: bool, is_meta_user: bool) -> Option<SpecialTurn> {
    let body = text.trim();
    if body.is_empty() {
        return None;
    }
    let delivered = is_meta_user && body.to_lowercase().contains("<scheduled-task");
    if !queued && !delivered {
        return None;
    }
    let prefix = if queued { "scheduled-prompt-queued" } else { "scheduled-prompt" };
    Some(SpecialTurn::ScheduledPrompt {
        queued,
        summary: trim_text(body, 240),
        signature: format!("{prefix}:{}", short_hash(body)),
    })
}

/// Apply special-turn annotation to a freshly emitted turn. Providers stamp
/// `queue_operation` / `is_meta` hints into provider_meta; this pass converts
/// matching turns into system specials so they never open exchanges.
pub fn annotate_special(turn: &mut Turn) {
    if turn.special.is_some() {
        return;
    }
    let queued = turn
        .provider_meta
        .get("queue_operation")
        .map(|v| {
            let op = v.as_str().unwrap_or("");
            op.is_empty() || op.eq_ignore_ascii_case("enqueue")
        })
        .unwrap_or(false)
        && turn.provider_meta.contains_key("queue_operation");
    let is_meta_user =
        turn.role == Role::User && turn.provider_meta.get("is_meta").and_then(|v| v.as_bool()) == Some(true);

    if let Some(special) = task_notification_special(&turn.text, queued) {
        let summary = match &special {
            SpecialTurn::TaskNotification { summary, .. } => summary.clone(),
            _ => String::new(),
        };
        turn.special = Some(special);
        turn.role = Role::System;
        turn.speaker = "task_notification".into();
        if !summary.is_empty() {
            // Keep the substantive body searchable; the queued echo stays
            // compact because the delivered copy carries the payload.
            if queued {
                turn.text = format!("Task queued: {summary}");
            }
        }
        return;
    }
    if let Some(special) = scheduled_prompt_special(&turn.text, queued, is_meta_user) {
        turn.special = Some(special);
        turn.role = Role::System;
        turn.speaker = "scheduled_prompt".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTIF: &str = "<task-notification><task-id>t1</task-id><status>completed</status><summary>built the thing</summary><result>ok</result></task-notification>";

    #[test]
    fn task_notification_parsed() {
        let s = task_notification_special(NOTIF, false).unwrap();
        match &s {
            SpecialTurn::TaskNotification { queued, status, summary, signature } => {
                assert!(!queued);
                assert_eq!(status.as_deref(), Some("completed"));
                assert_eq!(summary, "built the thing");
                assert!(signature.starts_with("task-notification:"));
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn queued_and_delivered_signatures_differ() {
        let delivered = task_notification_special(NOTIF, false).unwrap();
        let queued = task_notification_special(NOTIF, true).unwrap();
        let sig = |s: &SpecialTurn| match s {
            SpecialTurn::TaskNotification { signature, .. } => signature.clone(),
            _ => unreachable!(),
        };
        assert_ne!(sig(&delivered), sig(&queued));
        // Same underlying event hash, distinct kind prefix.
        assert!(sig(&queued).starts_with("task-notification-queued:"));
    }

    #[test]
    fn trailing_content_defeats_standalone() {
        assert!(standalone_tagged_body(&format!("{NOTIF} and more"), "task-notification").is_none());
        assert!(standalone_tagged_body(NOTIF, "task-notification").is_some());
    }

    #[test]
    fn scheduled_prompt_requires_signal() {
        assert!(scheduled_prompt_special("hello", false, false).is_none());
        assert!(scheduled_prompt_special("run nightly", true, false).is_some());
        assert!(scheduled_prompt_special("<scheduled-task name=\"x\">go</scheduled-task>", false, true).is_some());
        // A plain meta user prompt (caveat text etc.) is NOT a scheduled prompt.
        assert!(scheduled_prompt_special("Caveat: local commands", false, true).is_none());
    }
}
