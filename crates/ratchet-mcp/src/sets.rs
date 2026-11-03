//! The two tool sets of plan section 4.2. A session sees exactly one of them, chosen by its capability token.
//!
//! No tool here writes the evaluator, the protected set, budgets or other optimizations.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::args::*;
use crate::tool::Tool;

/// Which tools a session gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolSet {
    /// A chat: read state, draft and create optimizations, steer, open a PR.
    Chat,
    /// An optimization agent: steering notes, history, connector queries, candidates.
    Optimization,
}

impl ToolSet {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Optimization => "optimization",
        }
    }

    pub fn tools(self) -> Vec<Tool> {
        match self {
            Self::Chat => chat_tools(),
            Self::Optimization => optimization_tools(),
        }
    }

    pub fn names(self) -> Vec<String> {
        self.tools().into_iter().map(|t| t.name).collect()
    }

    pub fn find(self, name: &str) -> Option<Tool> {
        self.tools().into_iter().find(|t| t.name == name)
    }

    /// What the server tells the model about itself.
    pub fn instructions(self) -> &'static str {
        match self {
            Self::Chat => {
                "Ratchet's tools. Propose an optimization with draft_optimization and show the user the card; call \
                 create_optimization only after the user says to create it. Nothing here edits files."
            }
            Self::Optimization => {
                "Ratchet's tools for an optimization agent: read the user's steering notes at the start of each turn, \
                 look at the history before you try something again, and submit each candidate you commit."
            }
        }
    }
}

/// The JSON Schema of `T` as an object schema: sub-schemas inlined, no `$schema` or `title`, so any client can read it.
pub fn schema_of<T: JsonSchema>() -> Value {
    let settings = SchemaSettings::draft2020_12().with(|s| s.inline_subschemas = true);
    let schema = settings.into_generator().into_root_schema_for::<T>();
    let mut v = serde_json::to_value(schema).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("$schema");
        o.remove("title");
        o.entry("type")
            .or_insert_with(|| Value::String("object".into()));
        o.entry("properties")
            .or_insert_with(|| Value::Object(Default::default()));
    }
    v
}

fn check<T: DeserializeOwned>(arguments: &Value) -> Result<(), String> {
    serde_json::from_value::<T>(arguments.clone())
        .map(|_| ())
        .map_err(|e| format!("bad arguments: {e}"))
}

struct Spec {
    read_only: bool,
    destructive: bool,
    open_world: bool,
}

const READ: Spec = Spec {
    read_only: true,
    destructive: false,
    open_world: false,
};
const WRITE: Spec = Spec {
    read_only: false,
    destructive: false,
    open_world: false,
};

fn tool<T: JsonSchema + DeserializeOwned>(name: &str, description: &str, spec: Spec) -> Tool {
    Tool {
        name: name.into(),
        description: description.into(),
        input_schema: schema_of::<T>(),
        read_only: spec.read_only,
        destructive: spec.destructive,
        open_world: spec.open_world,
        validate: Some(check::<T>),
    }
}

pub fn chat_tools() -> Vec<Tool> {
    let net = |s: Spec| Spec {
        open_world: true,
        ..s
    };
    vec![
        tool::<ListOptimizationsArgs>(
            "list_optimizations",
            "List the optimizations with status, best result, spend and return.",
            READ,
        ),
        tool::<OptimizationArgs>(
            "get_optimization",
            "One optimization: agents, baseline vs best, calibration, pipeline.",
            READ,
        ),
        tool::<RunArgs>(
            "get_run",
            "The run of an optimization: every candidate with its verdict, and the spend by category.",
            READ,
        ),
        tool::<OverviewArgs>(
            "get_overview",
            "Spent, value locked in, return, and what needs the user.",
            READ,
        ),
        tool::<ListTemplatesArgs>(
            "list_templates",
            "The evaluator templates (and demos) a new optimization can start from.",
            READ,
        ),
        tool::<NoArgs>(
            "list_connectors",
            "The saved GitHub, AWS and agent connectors, with their status.",
            READ,
        ),
        tool::<ListReposArgs>(
            "list_repos",
            "The signed-in GitHub account's repos.",
            net(READ),
        ),
        tool::<DraftOptimizationArgs>(
            "draft_optimization",
            "Build a draft optimization and return its card. The config is committed to the repo's `ratchet/config` \
             branch, never to main. Nothing runs until create_optimization.",
            net(WRITE),
        ),
        tool::<CreateOptimizationArgs>(
            "create_optimization",
            "Create the optimization from a draft and start it. Call this only after the user asked for it.",
            WRITE,
        ),
        tool::<OptimizationArgs>("run_now", "Start an optimization now.", WRITE),
        tool::<OptimizationArgs>(
            "pause",
            "Pause every agent of an optimization after its current step.",
            WRITE,
        ),
        tool::<OptimizationArgs>(
            "resume",
            "Resume the paused agents of an optimization.",
            WRITE,
        ),
        tool::<SteeringNoteArgs>(
            "add_steering_note",
            "Send the agents of an optimization a note; they read it at the start of their next turn.",
            WRITE,
        ),
        tool::<OpenPrArgs>(
            "open_pr",
            "Open the pull request with an optimization's verified wins.",
            net(WRITE),
        ),
        tool::<RunArgs>(
            "explain_result",
            "The report of a run: bench numbers, guardrails and diffs, for you to explain to the user.",
            READ,
        ),
    ]
}

pub fn optimization_tools() -> Vec<Tool> {
    vec![
        tool::<NoArgs>(
            "steering_notes",
            "The user's notes since you last asked. Read them at the start of every turn.",
            READ,
        ),
        tool::<HistoryArgs>(
            "history",
            "Past candidates with their verdicts, and the current best.",
            READ,
        ),
        tool::<ConnectorQueryArgs>(
            "connector_query",
            "A read-only query against one of the user's connectors (metrics, logs, traces). Returns data, never credentials.",
            Spec {
                open_world: true,
                ..READ
            },
        ),
        tool::<SubmitCandidateArgs>(
            "submit_candidate",
            "Tell Ratchet a commit on your branch is a candidate for evaluation.",
            WRITE,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_sets_are_disjoint_and_named() {
        let chat = ToolSet::Chat.names();
        let opt = ToolSet::Optimization.names();
        assert_eq!(chat.len(), 15);
        assert_eq!(
            opt,
            [
                "steering_notes",
                "history",
                "connector_query",
                "submit_candidate"
            ]
        );
        assert!(chat.iter().all(|n| !opt.contains(n)));
    }

    #[test]
    fn schemas_are_inlined_object_schemas_from_the_core_types() {
        let t = ToolSet::Chat.find("draft_optimization").unwrap();
        let s = &t.input_schema;
        assert_eq!(s["type"], "object");
        assert!(
            s.get("$defs").is_none() && s.get("$schema").is_none(),
            "{s}"
        );
        assert_eq!(s["required"], serde_json::json!(["repo", "goal"]));
        // the agent enum is ratchet-core's AgentKind
        let agents = serde_json::to_string(&s["properties"]["agents"]).unwrap();
        assert!(
            agents.contains("claude-code") && agents.contains("codex"),
            "{agents}"
        );
    }

    #[test]
    fn arguments_are_checked_and_unknown_fields_refused() {
        let t = ToolSet::Chat.find("add_steering_note").unwrap();
        let v = t.validate.unwrap();
        assert!(v(&serde_json::json!({"optimization": "o", "text": "t"})).is_ok());
        assert!(v(&serde_json::json!({"optimization": "o"})).is_err());
        assert!(v(&serde_json::json!({"optimization": "o", "text": "t", "extra": 1})).is_err());
    }

    #[test]
    fn read_only_marks() {
        for t in ToolSet::Chat.tools() {
            let writes = [
                "draft_optimization",
                "create_optimization",
                "run_now",
                "pause",
                "resume",
                "add_steering_note",
                "open_pr",
            ];
            assert_eq!(
                t.read_only,
                !writes.contains(&t.name.as_str()),
                "{}",
                t.name
            );
        }
    }
}
