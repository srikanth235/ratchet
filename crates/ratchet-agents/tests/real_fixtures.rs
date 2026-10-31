//! Drop transcripts captured from the real CLIs (docs/real-env/agents.md) into `tests/fixtures/` as `real-claude-*.jsonl` /
//! `real-codex-*.jsonl`; they are checked here with no further test code. With none present the test passes without checking anything.

use ratchet_core::ports::AgentEventType;
use std::path::Path;

use ratchet_agents::{ClaudeStreamParser, CodexStreamParser};

#[test]
fn transcripts_captured_from_the_real_clis_parse_completely() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| {
            (f.starts_with("real-claude-") || f.starts_with("real-codex-")) && f.ends_with(".jsonl")
        })
        .collect();
    names.sort();
    for file in names {
        let text = std::fs::read_to_string(dir.join(&file)).unwrap();
        if file.starts_with("real-claude-") {
            let mut p = ClaudeStreamParser::new(|| 0);
            let mut events = p.push(&text);
            events.extend(p.end());
            assert!(p.session_id.is_some(), "{file}: session id");
            assert!(p.result.is_some(), "{file}: result line");
            assert!(
                events.iter().any(|e| e.event_type == AgentEventType::Usage),
                "{file}: usage"
            );
            assert!(
                p.unknown_types.is_empty(),
                "{file}: unknown event types: review them: {:?}",
                p.unknown_types
            );
        } else {
            let mut p = CodexStreamParser::new(|| 0, None);
            let mut events = p.push(&text);
            events.extend(p.end());
            assert!(p.thread_id.is_some(), "{file}: thread id");
            assert!(p.turn.completed || p.turn.failed, "{file}: turn outcome");
            assert!(!events.is_empty(), "{file}: events");
            assert!(
                p.unknown_types.is_empty(),
                "{file}: unknown event types: review them: {:?}",
                p.unknown_types
            );
        }
    }
}
