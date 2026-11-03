//! Fixture hygiene: every file in `tests/fixtures/` is named `<cli>-<version>-<real|mock>-<behaviour>.jsonl`, and transcripts
//! recorded from the real CLIs (`scripts/record-fixtures.sh`, marked `real`) hold no secret and parse with nothing left over.
//!
//! `real` means the real CLI emitted it; `mock` means it was written from docs or source. A recorded transcript is dropped into
//! `tests/fixtures/` and checked here with no further test code.

use ratchet_core::ports::{AgentEvent, AgentEventType};
use std::path::{Path, PathBuf};

use ratchet_agents::{ClaudeStreamParser, CodexStreamParser, GrokStreamParser};

/// The CLIs with fixtures. A fourth harness adds its name here (the test-only `echo` harness is one).
const CLIS: [&str; 4] = ["claude", "codex", "grok", "echo"];

struct Name<'a> {
    cli: &'a str,
    real: bool,
}

/// `<cli>-<version>-<real|mock>-<behaviour>.jsonl`; `None` when the name does not follow the scheme.
fn parse_name(file: &str) -> Option<Name<'_>> {
    let stem = file.strip_suffix(".jsonl")?;
    let (cli, rest) = stem.split_once('-')?;
    if !CLIS.contains(&cli) {
        return None;
    }
    let (version, rest) = rest.split_once('-')?;
    let parts: Vec<&str> = version.split('.').collect();
    let numeric = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if !numeric {
        return None;
    }
    let (kind, behaviour) = rest.split_once('-')?;
    let kebab = !behaviour.is_empty()
        && !behaviour.starts_with('-')
        && !behaviour.ends_with('-')
        && !behaviour.contains("--")
        && behaviour
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !kebab {
        return None;
    }
    match kind {
        "real" => Some(Name { cli, real: true }),
        "mock" => Some(Name { cli, real: false }),
        _ => None,
    }
}

/// The fixtures directory. `RATCHET_FIXTURES_DIR` points the real-transcript checks at fresh recordings instead
/// (`scripts/record-fixtures.sh --out DIR`): the nightly canary does that, and a failure there is CLI drift.
fn fixtures_dir() -> PathBuf {
    std::env::var_os("RATCHET_FIXTURES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"))
}

fn fixture_files() -> Vec<String> {
    let dir = fixtures_dir();
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn read(file: &str) -> String {
    std::fs::read_to_string(fixtures_dir().join(file)).unwrap()
}

#[test]
fn the_name_scheme_accepts_and_rejects_the_right_names() {
    for ok in [
        "claude-2.1.316-mock-session.jsonl",
        "claude-2.1.316-real-deltas.jsonl",
        "codex-0.166.0-mock-app-server-rate-limit.jsonl",
        "echo-0.1.0-mock-start.jsonl",
    ] {
        assert!(parse_name(ok).is_some(), "{ok}");
    }
    for bad in [
        "claude-success.jsonl",
        "claude-2.1-mock-session.jsonl",
        "claude-2.1.316-session.jsonl",
        "claude-2.1.316-fake-session.jsonl",
        "claude-2.1.316-mock-Session.jsonl",
        "claude-2.1.316-mock-.jsonl",
        "gemini-1.0.0-mock-session.jsonl",
        "real-claude-success.jsonl",
        "claude-2.1.316-mock-session.json",
    ] {
        assert!(parse_name(bad).is_none(), "{bad}");
    }
}

#[test]
fn every_fixture_name_carries_its_cli_version_and_whether_it_is_real() {
    let bad: Vec<String> = fixture_files()
        .into_iter()
        .filter(|f| parse_name(f).is_none())
        .collect();
    assert!(
        bad.is_empty(),
        "rename to <cli>-<version>-<real|mock>-<behaviour>.jsonl: {bad:?}"
    );
}

/// Event types the real CLIs send that the parsers deliberately ignore (reviewed; `harness_fixtures.rs` pins them). Anything else
/// in a real transcript is drift: the canary fails on it.
const KNOWN_IGNORED: [&str; 1] = ["system:status"];

fn unexpected(unknown: &std::collections::BTreeSet<String>) -> Vec<&String> {
    unknown
        .iter()
        .filter(|t| !KNOWN_IGNORED.contains(&t.as_str()))
        .collect()
}

/// Strings that look like a credential. A recorded transcript with one must not be committed.
fn secret_in(text: &str) -> Option<String> {
    let markers = [
        "sk-ant-",
        "sk-proj-",
        "xai-",
        "ghp_",
        "github_pat_",
        "AKIA",
        "Bearer ",
        "-----BEGIN",
        "api_key\":\"",
        "apiKey\":\"",
        "access_token\":\"",
        "refresh_token\":\"",
    ];
    markers
        .iter()
        .find(|m| text.contains(*m))
        .map(|m| (*m).to_string())
}

#[test]
fn real_fixtures_hold_no_secret() {
    for file in fixture_files() {
        if parse_name(&file).is_some_and(|n| n.real) {
            assert_eq!(secret_in(&read(&file)), None, "{file}");
        }
    }
    // the check itself works
    assert!(secret_in("{\"k\":\"sk-ant-abc\"}").is_some());
    assert!(secret_in("{\"auth\":\"Bearer abc\"}").is_some());
    assert!(secret_in("{\"text\":\"hello\"}").is_none());
}

/// What the parser of `cli` made of a transcript.
struct Parsed {
    events: Vec<AgentEvent>,
    /// The CLI named its session / thread.
    has_session: bool,
    /// The transcript ended the way a finished turn does (a `result` line, `turn.completed` / `turn.failed`).
    finished: bool,
    unknown: std::collections::BTreeSet<String>,
}

fn parse(cli: &str, text: &str, behaviour: &str) -> Parsed {
    match cli {
        "claude" => {
            let mut p = ClaudeStreamParser::new(|| 0);
            let mut events = p.push(text);
            events.extend(p.end());
            Parsed {
                events,
                has_session: p.session_id.is_some(),
                finished: p.result.is_some(),
                unknown: p.unknown_types,
            }
        }
        "codex" => {
            let mut p = CodexStreamParser::new(|| 0, None);
            let mut events = p.push(text);
            events.extend(p.end());
            Parsed {
                events,
                has_session: p.thread_id.is_some(),
                finished: p.turn.completed || p.turn.failed,
                unknown: p.unknown_types,
            }
        }
        "grok" => {
            let mut p = GrokStreamParser::new(|| 0);
            p.emit_deltas = behaviour == "deltas";
            let mut events = p.push(text);
            events.extend(p.end());
            Parsed {
                events,
                has_session: p.session_id.is_some(),
                finished: true,
                unknown: p.unknown_types,
            }
        }
        other => panic!("no parser for {other}"),
    }
}

fn count(events: &[AgentEvent], t: AgentEventType) -> usize {
    events.iter().filter(|e| e.event_type == t).count()
}

/// The shared contract, checked on what a real CLI printed. `behaviour` is the last part of the file name.
fn check_real(file: &str, cli: &str, behaviour: &str, text: &str) {
    let p = parse(cli, text, behaviour);
    assert!(!p.events.is_empty(), "{file}: events");
    assert!(
        unexpected(&p.unknown).is_empty(),
        "{file}: unknown event types: review them: {:?}",
        p.unknown
    );
    if behaviour == "auth-expired" {
        // a rejected key is one `auth_expired` event and nothing that would make the fallback layer wait
        assert_eq!(
            count(&p.events, AgentEventType::AuthExpired),
            1,
            "{file}: auth_expired"
        );
        assert_eq!(
            count(&p.events, AgentEventType::RateLimited),
            0,
            "{file}: rate_limited"
        );
        return;
    }
    assert!(p.has_session, "{file}: session id");
    assert!(p.finished, "{file}: turn outcome");
    assert!(
        count(&p.events, AgentEventType::Message) >= 1,
        "{file}: a message"
    );
    assert!(
        count(&p.events, AgentEventType::Usage) >= 1,
        "{file}: usage"
    );
    assert_eq!(
        count(&p.events, AgentEventType::Error),
        0,
        "{file}: error events"
    );
    if behaviour == "deltas" {
        let deltas: Vec<&AgentEvent> = p
            .events
            .iter()
            .filter(|e| e.event_type == AgentEventType::Delta)
            .collect();
        assert!(!deltas.is_empty(), "{file}: deltas");
        let message = p
            .events
            .iter()
            .find(|e| e.event_type == AgentEventType::Message)
            .and_then(|e| e.text.clone())
            .unwrap_or_default();
        let joined: String = deltas.iter().filter_map(|e| e.text.clone()).collect();
        assert!(
            message.starts_with(&joined) || joined == message,
            "{file}: deltas add up to the message"
        );
    }
}

#[test]
fn transcripts_recorded_from_the_real_clis_parse_completely_and_keep_the_contract() {
    for file in fixture_files() {
        let Some(name) = parse_name(&file).filter(|n| n.real) else {
            continue;
        };
        let stem = file.trim_end_matches(".jsonl");
        // <cli>-<version>-real-<behaviour>
        let behaviour = stem
            .splitn(4, '-')
            .nth(3)
            .unwrap_or_else(|| panic!("{file}: behaviour"));
        check_real(&file, name.cli, behaviour, &read(&file));
    }
}

#[test]
fn the_real_transcript_check_catches_drift() {
    // a new event type from a CLI update is drift
    let drifted = "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}\n{\"type\":\"brand_new_kind\"}\n";
    let p = parse("claude", drifted, "success");
    assert_eq!(unexpected(&p.unknown), ["brand_new_kind"]);
    // an auth failure with no classified event is drift
    let result = std::panic::catch_unwind(|| {
        check_real(
            "x",
            "claude",
            "auth-expired",
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}\n",
        )
    });
    assert!(result.is_err());
}
