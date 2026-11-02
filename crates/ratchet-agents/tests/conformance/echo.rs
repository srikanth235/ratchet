//! A fourth harness that exists only for tests: `echo-cli`, a made-up agent CLI with the simplest possible JSONL protocol.
//!
//! It is here to prove the extension story of the harness layer: a new CLI needs an adapter (this file) and fixtures
//! (`tests/fixtures/echo-0.1.0-mock-*.jsonl`), and the conformance suite in `tests/conformance.rs` runs it without a single
//! scenario being edited. Nothing under `src/` knows about it.
//!
//! The protocol, one JSON object per stdout line (one process per turn):
//!
//! ```text
//! {"t":"session","id":"echo-session-1"}                         the session id
//! {"t":"delta","text":"Hel"}                                    partial text
//! {"t":"message","text":"Hello"}                                the whole message
//! {"t":"tool","phase":"call","id":"c1","name":"x","input":{}}   a tool call
//! {"t":"tool","phase":"result","id":"c1","text":"ok","error":false}
//! {"t":"usage","input":10,"output":4,"cost":0.001}
//! {"t":"limit","resets_at":1793614200}                          out of quota (resets_at is optional)
//! {"t":"auth"}                                                  login rejected
//! {"t":"done"}                                                  the turn ended normally
//! ```
//!
//! Command line: `echo-cli run [--resume ID] [--read-only] [--mcp NAME=COMMAND]...`, the prompt on stdin.

use std::sync::Arc;

use async_trait::async_trait;
use ratchet_agents::classify::{FailureKind, failure_event};
use ratchet_agents::{CliAgent, CliAgentOptions, SpawnPerTurnSession, TurnRunner};
use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{
    Access, AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, AgentUsage,
    Capabilities, EventSink, Harness, LiveSession, PortError, PortResult, Preflight, SessionSpec,
    SharedEventSink,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub struct EchoHarness {
    cli: CliAgent,
}

impl EchoHarness {
    pub fn new(cli: CliAgentOptions) -> Self {
        Self {
            cli: CliAgent::new("echo-cli", cli),
        }
    }

    fn args(spec: &SessionSpec, resume: Option<&str>) -> Vec<String> {
        let mut args = vec!["run".to_string()];
        if let Some(id) = resume {
            args.extend(["--resume".to_string(), id.to_string()]);
        }
        if spec.access == Access::ReadOnly {
            args.push("--read-only".to_string());
        }
        for server in &spec.mcp {
            args.extend([
                "--mcp".to_string(),
                format!("{}={}", server.name, server.command),
            ]);
        }
        args
    }
}

#[async_trait]
impl TurnRunner for EchoHarness {
    fn kind(&self) -> AgentKind {
        AgentKind::Mock
    }

    async fn run_turn(
        &self,
        session: &mut AgentSession,
        spec: &SessionSpec,
        resume: Option<&str>,
        prompt: &str,
        events: &EventSink<'_>,
        cancel: &CancellationToken,
    ) -> PortResult<()> {
        let now = || self.cli.clock.now();
        let mut failure: Option<FailureKind> = None;
        let mut done = false;
        let mut id = None;
        let mut on_line = |line: &str| {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return;
            };
            let text = v["text"].as_str().unwrap_or_default();
            match v["t"].as_str().unwrap_or_default() {
                "session" => id = v["id"].as_str().map(str::to_string),
                "delta" => {
                    events(AgentEvent::new(now(), AgentEventType::Delta).with_text(text));
                }
                "message" => {
                    events(AgentEvent::new(now(), AgentEventType::Message).with_text(text));
                }
                "tool" => {
                    let call = v["phase"] == "call";
                    let mut data = json!({"phase": v["phase"], "id": v["id"]});
                    if call {
                        data["name"] = v["name"].clone();
                        data["input"] = v["input"].clone();
                    } else {
                        data["isError"] = json!(v["error"].as_bool().unwrap_or(false));
                    }
                    let shown = if call {
                        v["name"].as_str().unwrap_or_default()
                    } else {
                        text
                    };
                    events(
                        AgentEvent::new(now(), AgentEventType::Tool)
                            .with_text(shown)
                            .with_data(data),
                    );
                }
                "usage" => {
                    events(
                        AgentEvent::new(now(), AgentEventType::Usage).with_usage(AgentUsage {
                            input_tokens: v["input"].as_u64().unwrap_or(0),
                            output_tokens: v["output"].as_u64().unwrap_or(0),
                            cost_usd: v["cost"].as_f64(),
                            ..AgentUsage::default()
                        }),
                    );
                }
                "limit" => {
                    let resets_at = v["resets_at"].as_i64();
                    failure = Some(FailureKind::RateLimited { resets_at });
                    events(failure_event(
                        now(),
                        FailureKind::RateLimited { resets_at },
                        "out of quota",
                    ));
                }
                "auth" => {
                    failure = Some(FailureKind::AuthExpired);
                    events(failure_event(
                        now(),
                        FailureKind::AuthExpired,
                        "login rejected",
                    ));
                }
                "done" => done = true,
                _ => {}
            }
        };
        let res = self
            .cli
            .run_in(
                &Self::args(spec, resume),
                &spec.workdir,
                spec.env.as_ref(),
                Some(prompt.to_string()),
                &mut on_line,
                cancel,
            )
            .await;
        if let Some(id) = id.filter(|i| !i.is_empty()) {
            session.id = id;
        }
        if res.aborted {
            return Err(PortError::Cancelled);
        }
        let fail = |message: String| {
            events(
                AgentEvent::new(now(), AgentEventType::Error)
                    .with_text(message.clone())
                    .with_data(json!({"exitCode": res.code()})),
            );
            PortError::AgentFailed {
                message,
                exit_code: res.code(),
                stderr: Some(res.stderr_tail.clone()),
            }
        };
        if failure.is_some() {
            // the classified event is already out
            return Err(PortError::AgentFailed {
                message: "echo-cli refused the turn".to_string(),
                exit_code: res.code(),
                stderr: Some(res.stderr_tail.clone()),
            });
        }
        if !done {
            return Err(fail(format!(
                "echo-cli exited with code {:?} mid-turn",
                res.code()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl AgentAdapter for EchoHarness {
    fn kind(&self) -> AgentKind {
        AgentKind::Mock
    }

    async fn preflight(&self) -> PortResult<Preflight> {
        Ok(Preflight {
            ok: true,
            problems: vec![],
        })
    }

    async fn start(
        &self,
        spec: &AgentSpec,
        events: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<AgentSession> {
        let spec = SessionSpec::from(spec);
        let mut session = AgentSession {
            id: String::new(),
            kind: AgentKind::Mock,
        };
        let cancel = cancel.cloned().unwrap_or_default();
        self.run_turn(&mut session, &spec, None, &spec.prompt, events, &cancel)
            .await?;
        Ok(session)
    }

    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        events: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<()> {
        let spec = SessionSpec::from(spec);
        let cancel = cancel.cloned().unwrap_or_default();
        let id = session.id.clone();
        self.run_turn(session, &spec, Some(&id), message, events, &cancel)
            .await
    }
}

#[async_trait]
impl Harness for EchoHarness {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: true,
            live_session: false,
            mcp: true,
            cost_in_usd: true,
            read_only: true,
        }
    }

    async fn open(
        &self,
        spec: &SessionSpec,
        events: SharedEventSink,
    ) -> PortResult<Box<dyn LiveSession>> {
        let runner: Arc<dyn TurnRunner> = Arc::new(Self {
            cli: self.cli.clone(),
        });
        let session = SpawnPerTurnSession::new(runner, spec.clone(), events);
        if !spec.prompt.is_empty() {
            session.send(&spec.prompt).await?;
        }
        Ok(Box::new(session))
    }
}
