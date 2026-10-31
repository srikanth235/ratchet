//! String enums shared by the config, the ledger and the API. Each serializes to the exact string the
//! TypeScript code and the SQLite `CHECK` constraints use, and has `as_str`, `Display`, `FromStr` and `ALL`.

use std::fmt;
use std::str::FromStr;

/// Returned by `FromStr` for the enums in this module.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} {value:?}")]
pub struct ParseEnumError {
    pub kind: &'static str,
    pub value: String,
}

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $s:tt ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord,
            serde::Serialize, serde::Deserialize, schemars::JsonSchema,
        )]
        #[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
        pub enum $name {
            $( $(#[$vmeta])* #[serde(rename = $s)] $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];

            /// The wire / database string.
            pub const fn as_str(self) -> &'static str {
                match self { $( $name::$variant => $s ),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $( $s => Ok($name::$variant), )+
                    other => Err(ParseEnumError { kind: stringify!($name), value: other.to_owned() }),
                }
            }
        }
    };
}

string_enum!(
    /// Which way the objective metric improves.
    Direction {
        Minimize => "minimize",
        Maximize => "maximize",
    }
);

string_enum!(
    /// How much infrastructure an optimization's environment needs (`environment.scope`, docs/SIMPLIFIED_MODEL.md,
    /// "Dynamic environments"). `none`: pure code / tests, the evaluator's container is all there is; `single`: one
    /// machine on pinned hardware; `full`: a stack (app + databases + seeded data + a load generator) written as
    /// OpenTofu under `infra/ratchet/` on top of the base module Ratchet ships.
    EnvironmentScope {
        None => "none",
        Single => "single",
        Full => "full",
    }
);

string_enum!(
    /// What kind of trusted evaluation produces the objective metric.
    EvaluatorKind {
        Benchmark => "benchmark",
        Backtest => "backtest",
        Eval => "eval",
        Simulation => "simulation",
    }
);

string_enum!(
    /// How the candidate's dependencies reach the offline bench (`evaluator.deps`).
    ///
    /// `auto` detects the package manager from the lockfile at `evaluator.ref`; `none` installs nothing.
    DepsMode {
        Auto => "auto",
        Pnpm => "pnpm",
        Npm => "npm",
        None => "none",
    }
);

string_enum!(
    /// The period a value-model amount is quoted for.
    ValuePeriod {
        Month => "month",
        Year => "year",
    }
);

string_enum!(
    /// `estimated` until the value has been measured in production.
    ValueBasis {
        Estimated => "estimated",
        Measured => "measured",
    }
);

string_enum!(
    /// Who runs a sandbox: the AWS tofu provider, or the simulation.
    SandboxProviderKind {
        Aws => "aws",
        Mock => "mock",
    }
);

string_enum!(
    /// Sandbox lifecycle state recorded in the ledger.
    SandboxStatus {
        Planned => "planned",
        Applying => "applying",
        Applied => "applied",
        Destroying => "destroying",
        Destroyed => "destroyed",
        Failed => "failed",
    }
);

string_enum!(
    /// Optimization lifecycle state.
    OptimizationStatus {
        Draft => "draft",
        Baselining => "baselining",
        Running => "running",
        Stopping => "stopping",
        Completed => "completed",
        Stopped => "stopped",
        Failed => "failed",
    }
);

impl OptimizationStatus {
    /// Baselining, running or stopping: a run is using its sandbox.
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Baselining | Self::Running | Self::Stopping)
    }

    /// Completed, stopped or failed.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Stopped | Self::Failed)
    }
}

string_enum!(
    /// Which agent CLI (or the mock) an agent runs.
    AgentKind {
        ClaudeCode => "claude-code",
        Codex => "codex",
        Mock => "mock",
    }
);

string_enum!(
    /// Agent lifecycle state.
    AgentStatus {
        Pending => "pending",
        Launching => "launching",
        Running => "running",
        Paused => "paused",
        Stopped => "stopped",
        Done => "done",
        Failed => "failed",
    }
);

string_enum!(
    /// A candidate's verdict as stored in the ledger: `pending` until the bench runner decides, the rest
    /// mirror the ratchet rule.
    CandidateVerdict {
        Pending => "pending",
        Verified => "verified",
        Rejected => "rejected",
        FailedConfirm => "failed_confirm",
        Blocked => "blocked",
    }
);

string_enum!(
    /// The outcome of the ratchet rule (`ratchet::evaluate_ratchet`).
    Verdict {
        /// All gates passed; the ratchet may advance.
        Verified => "verified",
        /// Guardrails ok, but no gain beyond the threshold.
        Rejected => "rejected",
        /// A bench win that did not hold on the hidden trace.
        FailedConfirm => "failed_confirm",
        /// A guardrail failed or a protected path was touched.
        Blocked => "blocked",
        /// A bench win, waiting for the confirm run.
        PendingConfirm => "pending_confirm",
    }
);

impl Verdict {
    /// The ledger verdict for this outcome (`pending_confirm` is still `pending` in the ledger).
    pub const fn to_candidate_verdict(self) -> CandidateVerdict {
        match self {
            Verdict::Verified => CandidateVerdict::Verified,
            Verdict::Rejected => CandidateVerdict::Rejected,
            Verdict::FailedConfirm => CandidateVerdict::FailedConfirm,
            Verdict::Blocked => CandidateVerdict::Blocked,
            Verdict::PendingConfirm => CandidateVerdict::Pending,
        }
    }
}

string_enum!(
    /// Phase of a bench result.
    BenchPhase {
        Bench => "bench",
        Confirm => "confirm",
    }
);

string_enum!(
    /// Spend categories of the ledger. `tokens` are model API spend, which AWS does not bill.
    SpendCategory {
        AgentRunner => "agent_runner",
        Experiment => "experiment",
        BenchRunner => "bench_runner",
        StorageEgress => "storage_egress",
        Tokens => "tokens",
    }
);

string_enum!(
    /// Whether the engine talks to real systems or to the simulation.
    Mode {
        Live => "live",
        Simulation => "simulation",
    }
);

string_enum!(
    /// Where an optimization's agents run: this machine, or cloud runners.
    AgentsWhere {
        Local => "local",
        Cloud => "cloud",
    }
);

string_enum!(
    /// Whether a template is a real, runnable evaluator or a "Try a demo" entry.
    TemplateKind {
        Template => "template",
        Demo => "demo",
    }
);

string_enum!(
    /// Gallery grouping of templates (formerly "domain packs").
    TemplateCategory {
        Perf => "perf",
        Forecasting => "forecasting",
        Llm => "llm",
    }
);

string_enum!(
    /// Role of a tagged cloud instance.
    InstanceRole {
        AgentRunner => "agent_runner",
        Experiment => "experiment",
        Bench => "bench",
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_round_trip() {
        for v in CandidateVerdict::ALL {
            assert_eq!(v.as_str().parse::<CandidateVerdict>().unwrap(), *v);
            assert_eq!(
                serde_json::to_string(v).unwrap(),
                format!("\"{}\"", v.as_str())
            );
        }
        assert!("great".parse::<CandidateVerdict>().is_err());
        assert_eq!(AgentKind::ClaudeCode.to_string(), "claude-code");
    }

    #[test]
    fn verdict_maps_to_ledger_verdict() {
        assert_eq!(
            Verdict::PendingConfirm.to_candidate_verdict(),
            CandidateVerdict::Pending
        );
        assert_eq!(
            Verdict::FailedConfirm.to_candidate_verdict(),
            CandidateVerdict::FailedConfirm
        );
    }

    #[test]
    fn active_statuses() {
        let active: Vec<_> = OptimizationStatus::ALL
            .iter()
            .filter(|s| s.is_active())
            .collect();
        assert_eq!(
            active,
            [
                &OptimizationStatus::Baselining,
                &OptimizationStatus::Running,
                &OptimizationStatus::Stopping
            ]
        );
    }
}
