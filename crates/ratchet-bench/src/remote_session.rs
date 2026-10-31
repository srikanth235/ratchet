//! One launched instance and the plumbing every remote job needs (port of `remote-session.ts`): wait for SSM, run a script
//! raced against the cancellation token (output minus sentinel lines goes to the log) and terminate. Shared by the bench
//! executor and the dependency-snapshot job, which therefore have the same terminate-in-`finally` discipline.

use std::sync::Arc;

use ratchet_core::ports::{CancellationToken, Clock, CloudCompute, InstanceInfo, PortError, RemoteCommand, RemoteCommandResult};

use crate::errors::{BenchError, BenchResult};
use crate::log::BenchLog;
use crate::remote_scripts::{ping_script, strip_sentinels, wrap_step};

/// Called with non-fatal problems, e.g. a failed `terminate` (the reaper is the backstop).
pub type WarningSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct SessionOptions {
    pub compute: Arc<dyn CloudCompute>,
    pub clock: Arc<dyn Clock>,
    /// The first command is retried while the instance's SSM agent registers. Defaults: 30 attempts, 10 s apart.
    pub ready_attempts: Option<u32>,
    pub ready_delay_ms: Option<u64>,
    pub on_warning: Option<WarningSink>,
}

pub struct InstanceSession {
    o: SessionOptions,
    pub instance: InstanceInfo,
    log: BenchLog,
    cancel: Option<CancellationToken>,
    /// Used in log lines: `bench`, `dependency snapshot`.
    label: String,
}

impl InstanceSession {
    pub fn new(
        o: SessionOptions,
        instance: InstanceInfo,
        log: BenchLog,
        cancel: Option<CancellationToken>,
        label: &str,
    ) -> Self {
        InstanceSession {
            o,
            instance,
            log,
            cancel,
            label: label.to_owned(),
        }
    }

    pub async fn wait_ready(&self) -> BenchResult<()> {
        let attempts = self.o.ready_attempts.unwrap_or(30);
        let mut i = 1;
        loop {
            let attempt: BenchResult<()> = match self.call("ping", ping_script(), 60_000).await {
                Ok(r) if r.exit_code == 0 => return Ok(()),
                Ok(r) => Err(BenchError::infra(format!("exit {}", r.exit_code))),
                Err(e) => Err(e),
            };
            if let Err(err) = attempt {
                if err == BenchError::Aborted {
                    return Err(err);
                }
                if i >= attempts {
                    return Err(BenchError::infra(format!(
                        "{} instance {} never became ready: {err}",
                        self.label, self.instance.id
                    )));
                }
                self.o
                    .clock
                    .sleep(self.o.ready_delay_ms.unwrap_or(10_000), self.cancel.as_ref())
                    .await
                    .map_err(|e| match e {
                        PortError::Cancelled => BenchError::Aborted,
                        other => BenchError::infra(other.to_string()),
                    })?;
            }
            i += 1;
        }
    }

    /// One `run_command`, raced against the cancellation token; the output (minus sentinel lines) goes to the log.
    pub async fn call(&self, name: &str, script: &str, timeout_ms: u64) -> BenchResult<RemoteCommandResult> {
        if self.cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(BenchError::Aborted);
        }
        let command = RemoteCommand {
            script: wrap_step(script)?,
            env: None,
            timeout_seconds: Some(timeout_ms.div_ceil(1000) + 120),
        };
        let pending = self.o.compute.run_command(&self.instance.id, &command);
        let cancelled = async {
            match &self.cancel {
                Some(c) => c.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };
        let res = tokio::select! {
            r = pending => r.map_err(|e| BenchError::infra(format!(
                "runCommand \"{name}\" on {} failed: {e}",
                self.instance.id
            )))?,
            () = cancelled => return Err(BenchError::Aborted),
        };
        if name != "ping" {
            self.log
                .section(&format!("remote step: {name} (exit {})", res.exit_code));
            self.log.line(&strip_sentinels(&res.stdout));
            if !res.stderr.trim().is_empty() {
                self.log.line(&format!("[stderr]\n{}", res.stderr));
            }
        }
        Ok(res)
    }

    /// Terminates the instance; a failure is logged and reported, never returned (the reaper cleans up what slips through).
    pub async fn terminate(&self) {
        match self
            .o
            .compute
            .terminate(std::slice::from_ref(&self.instance.id))
            .await
        {
            Ok(()) => self.log.line(&format!(
                "{} instance {} terminated",
                self.label, self.instance.id
            )),
            Err(err) => {
                let msg = format!(
                    "FAILED to terminate {} instance {}: {err} (the reaper must clean it up)",
                    self.label, self.instance.id
                );
                self.log.line(&msg);
                if let Some(w) = &self.o.on_warning {
                    w(&msg);
                }
            }
        }
    }
}

/// The last `n` characters of `s.trim()`, with an ellipsis when it was longer.
pub fn tail(s: &str, n: usize) -> String {
    let t = s.trim();
    let count = t.chars().count();
    if count > n {
        format!("...{}", t.chars().skip(count - n).collect::<String>())
    } else {
        t.to_owned()
    }
}

/// `tail(s, 600)`.
pub fn tail600(s: &str) -> String {
    tail(s, 600)
}
