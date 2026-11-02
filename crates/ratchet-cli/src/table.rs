//! The command table: every `ratchet` command with its arguments and its safety mark, read from the clap definition in
//! [`crate::cli`]. `ratchet --help`, the shell parser and the MCP owner mode (`ratchet mcp`) all come from this one definition,
//! so they cannot drift. The only hand-written part is [`MARKS`], and a test fails when a command has no mark.

use std::any::TypeId;

use clap::{Arg, ArgAction, Command, CommandFactory};

use crate::cli::Cli;

/// What a command may do. Mirrors the agent-cli rules: read-only commands are safe to run any time; destructive ones only
/// preview until `--yes` (MCP: `confirm: true`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    ReadOnly,
    /// Changes data, can be undone or repeated.
    Write,
    /// Stops work, spends money or publishes: needs an explicit confirmation.
    Destructive,
}

/// Command path (space separated) and its mark. Every command of the CLI except `mcp` itself is listed here.
pub const MARKS: &[(&str, Mark)] = &[
    ("validate", Mark::ReadOnly),
    ("init", Mark::Write),
    ("baseline", Mark::Write),
    ("templates", Mark::ReadOnly),
    ("status", Mark::ReadOnly),
    ("list", Mark::ReadOnly),
    ("show", Mark::ReadOnly),
    ("run", Mark::Destructive),
    ("stop", Mark::Destructive),
    ("report", Mark::ReadOnly),
    ("pr", Mark::Destructive),
    ("candidates", Mark::ReadOnly),
    ("history", Mark::ReadOnly),
    ("spend", Mark::ReadOnly),
    ("activity", Mark::ReadOnly),
    ("agent pause", Mark::Write),
    ("agent resume", Mark::Write),
    ("agent message", Mark::Write),
    ("agent log", Mark::ReadOnly),
    ("agents show", Mark::ReadOnly),
    ("agents set", Mark::Write),
    ("events", Mark::ReadOnly),
    ("chat new", Mark::Write),
    ("chat send", Mark::Write),
    ("chat show", Mark::ReadOnly),
    ("chat stop", Mark::Write),
    ("chat end", Mark::Destructive),
    ("env list", Mark::ReadOnly),
    ("env end", Mark::Destructive),
    ("cost reconcile", Mark::Write),
    ("repos", Mark::ReadOnly),
    ("inspect", Mark::ReadOnly),
    ("overview", Mark::ReadOnly),
    ("home", Mark::ReadOnly),
    ("value show", Mark::ReadOnly),
    ("value set", Mark::Write),
    ("value clear", Mark::Destructive),
    ("schedule show", Mark::ReadOnly),
    ("schedule set", Mark::Write),
    ("schedule clear", Mark::Write),
    ("pr-merged", Mark::Write),
    ("connectors", Mark::ReadOnly),
    ("open", Mark::Write),
];

/// Arguments the MCP tools leave out: they write files on the machine running the server (the tool returns the text instead).
pub const HIDDEN_ARGS: &[(&str, &str)] = &[("report", "out")];

/// The command that is the server itself; it has no tool.
pub const SERVER_COMMAND: &str = "mcp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    String,
    Integer,
    Number,
    Boolean,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArgSpec {
    /// The JSON property name: the long flag with `_` for `-` (`base_ref`), or the clap id of a positional.
    pub name: String,
    /// `--base-ref`, or `None` for a positional.
    pub flag: Option<String>,
    pub kind: ArgKind,
    pub required: bool,
    /// Several values (a repeated flag).
    pub repeated: bool,
    pub choices: Vec<String>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandSpec {
    /// `["agent", "pause"]`.
    pub path: Vec<String>,
    pub summary: String,
    pub args: Vec<ArgSpec>,
    pub mark: Mark,
}

impl CommandSpec {
    /// `agent pause`.
    pub fn key(&self) -> String {
        self.path.join(" ")
    }

    /// The MCP tool name, `agent_pause`.
    pub fn tool_name(&self) -> String {
        self.path.join("_")
    }
}

pub fn mark_of(key: &str) -> Option<Mark> {
    MARKS.iter().find(|(k, _)| *k == key).map(|(_, m)| *m)
}

/// Every command of the CLI, in definition order, except the MCP server. Panics only if [`MARKS`] misses a command, which the
/// tests catch.
pub fn command_table() -> Vec<CommandSpec> {
    let root = Cli::command();
    let mut out = Vec::new();
    for sub in root.get_subcommands() {
        collect(sub, &mut Vec::new(), &mut out);
    }
    out
}

fn collect(cmd: &Command, prefix: &mut Vec<String>, out: &mut Vec<CommandSpec>) {
    let name = cmd.get_name();
    if name == "help" || cmd.is_hide_set() || (prefix.is_empty() && name == SERVER_COMMAND) {
        return;
    }
    prefix.push(name.to_owned());
    let subs: Vec<&Command> = cmd
        .get_subcommands()
        .filter(|c| c.get_name() != "help")
        .collect();
    if subs.is_empty() {
        out.push(spec_of(cmd, prefix));
    } else {
        for s in subs {
            collect(s, prefix, out);
        }
    }
    prefix.pop();
}

fn spec_of(cmd: &Command, path: &[String]) -> CommandSpec {
    let key = path.join(" ");
    let mark = mark_of(&key).unwrap_or(Mark::Destructive); // unmarked = most careful
    let mut positional: Vec<&Arg> = cmd
        .get_arguments()
        .filter(|a| a.is_positional() && !a.is_hide_set())
        .collect();
    positional.sort_by_key(|a| a.get_index());
    let flags = cmd.get_arguments().filter(|a| !a.is_positional());
    let args = positional
        .into_iter()
        .chain(flags)
        .filter(|a| !a.is_global_set() && !a.is_hide_set())
        .filter(|a| !matches!(a.get_id().as_str(), "help" | "version" | "yes"))
        .filter(|a| !HIDDEN_ARGS.contains(&(key.as_str(), a.get_id().as_str())))
        .map(arg_spec)
        .collect();
    CommandSpec {
        path: path.to_vec(),
        summary: cmd.get_about().map(ToString::to_string).unwrap_or_default(),
        args,
        mark,
    }
}

fn arg_spec(a: &Arg) -> ArgSpec {
    let action = a.get_action();
    let boolean = matches!(action, ArgAction::SetTrue | ArgAction::SetFalse);
    let tid = a.get_value_parser().type_id();
    let kind = if boolean {
        ArgKind::Boolean
    } else if [
        TypeId::of::<u8>(),
        TypeId::of::<u16>(),
        TypeId::of::<u32>(),
        TypeId::of::<u64>(),
        TypeId::of::<usize>(),
        TypeId::of::<i8>(),
        TypeId::of::<i16>(),
        TypeId::of::<i32>(),
        TypeId::of::<i64>(),
        TypeId::of::<isize>(),
    ]
    .iter()
    .any(|t| tid == *t)
    {
        ArgKind::Integer
    } else if tid == TypeId::of::<f64>() {
        ArgKind::Number
    } else {
        ArgKind::String
    };
    let choices = a
        .get_possible_values()
        .iter()
        .filter(|p| !p.is_hide_set())
        .map(|p| p.get_name().to_owned())
        .collect();
    ArgSpec {
        // a flag is named as on the command line (`--agent` is `agent`, `--base-ref` is `base_ref`)
        name: a
            .get_long()
            .map_or_else(|| a.get_id().to_string(), |l| l.replace('-', "_")),
        flag: a.get_long().map(str::to_owned),
        kind,
        required: a.is_required_set(),
        repeated: matches!(action, ArgAction::Append) || a.get_value_delimiter().is_some(),
        choices,
        description: a.get_help().map(ToString::to_string).unwrap_or_default(),
    }
}
