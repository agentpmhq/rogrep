//! Robustness: arbitrary input never panics, and structural invariants hold
//! on whatever does parse.

mod common;

use proptest::prelude::*;
use rogrep_model::AgentKind;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Arbitrary bytes: no panic, monotonic source spans, dense turn indexes.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Generic] {
            let out = common::parse_bytes(kind, "fuzz.jsonl", &bytes, None);
            let turns = &out.conversation.turns;
            for (i, t) in turns.iter().enumerate() {
                prop_assert_eq!(t.turn_index as usize, i);
            }
            for w in turns.windows(2) {
                prop_assert!(w[0].source.byte_start <= w[1].source.byte_start);
            }
        }
    }

    /// JSON-shaped garbage lines: parse must classify every line as either a
    /// turn source or malformed, never both, never crash.
    #[test]
    fn jsonish_lines_are_stable(
        lines in proptest::collection::vec(
            prop_oneof![
                Just(r#"{"type":"user","message":{"role":"user","content":"hi"}}"#.to_string()),
                Just(r#"{"type":"assistant","message":{"role":"assistant","content":"yo"}}"#.to_string()),
                Just("not json".to_string()),
                Just(r#"{"unfinished": "#.to_string()),
                Just(String::new()),
                Just(r#"{"type":"event_msg","payload":{"type":"error","message":"x"}}"#.to_string()),
            ],
            0..40,
        )
    ) {
        let bytes = lines.join("\n").into_bytes();
        let out = common::parse_bytes(AgentKind::Claude, "fuzz2.jsonl", &bytes, None);
        // Deterministic: parsing twice gives identical output.
        let out2 = common::parse_bytes(AgentKind::Claude, "fuzz2.jsonl", &bytes, None);
        prop_assert_eq!(out.conversation, out2.conversation);
        prop_assert_eq!(out.state, out2.state);
    }
}
