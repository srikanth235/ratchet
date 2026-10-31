//! The brief a setup agent is given (`brief.ts`). STUB.

use ratchet_core::enums::EnvironmentScope;

/// The marker the agent ends its final message with, followed by one JSON object.
pub const ANSWER_MARKER: &str = "RATCHET_ENVIRONMENT";
/// The canonical shape of `main.tf`, shown to the agent (and used by the mock drafter).
pub const MAIN_TF_SKELETON: &str = "";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAnswer {
    pub scope: EnvironmentScope,
    /// One or two plain sentences: why this much infrastructure. Shown to the user.
    pub reason: String,
}

pub fn format_agent_answer(_a: &AgentAnswer) -> String {
    todo!()
}

/// Reads the agent's final message. Tolerant: the marker may be anywhere and the JSON may be fenced; a bad answer
/// is `None`.
pub fn parse_agent_answer(_text: &str) -> Option<AgentAnswer> {
    todo!()
}

#[derive(Debug, Clone, Default)]
pub struct SetupBriefInput {
    /// What the user wants optimized, in their words.
    pub goal: String,
    /// The bench machine the repo's ratchet.yaml / template pins, when known.
    pub hardware: Option<String>,
    /// The run's time limit in seconds, so the agent can weigh cost.
    pub time_limit_seconds: Option<f64>,
}

pub fn build_setup_brief(_input: &SetupBriefInput) -> String {
    todo!()
}

pub fn build_revise_brief(_message: &str) -> String {
    todo!()
}
