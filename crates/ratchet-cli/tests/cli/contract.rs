//! The CLI contract that agents rely on (`.claude/skills/ratchet/SKILL.md`): `--help` lists every command, every command has
//! help of its own, `--json` output parses, a wrong id exits 1 with a hint, a command that spends money needs `--yes`, and
//! `ratchet open` builds the link the Mac app handles. `native.yml` runs the same checks on the CLI inside the app bundle
//! (`crates/ratchet-cli/tests/contract.sh`).

use clap::CommandFactory;
use ratchet_cli::cli::Cli;

use crate::common::{Env, Out, ratchet};

/// Every top-level command, as an agent knows it. Adding a command means adding it here and to the skill.
const COMMANDS: &[&str] = &[
    "validate",
    "init",
    "baseline",
    "templates",
    "status",
    "list",
    "show",
    "run",
    "stop",
    "report",
    "pr",
    "candidates",
    "history",
    "spend",
    "activity",
    "agent",
    "agents",
    "events",
    "chat",
    "env",
    "cost",
    "repos",
    "inspect",
    "overview",
    "home",
    "value",
    "schedule",
    "pr-merged",
    "connectors",
    "open",
    "mcp",
];

/// The command names in the "Commands:" block of `--help` output.
fn listed_in_help(help: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_block = false;
    for line in help.lines() {
        if line.starts_with("Commands:") {
            in_block = true;
            continue;
        }
        if in_block {
            if !line.starts_with("  ") {
                break;
            }
            // "  list, ls  List optimizations." or "  show  One optimization."
            if let Some(first) = line.split_whitespace().next() {
                names.push(first.trim_end_matches(',').to_owned());
            }
        }
    }
    names
}

/// Every command path of the clap definition, `["agent", "pause"]`, parents included.
fn command_paths() -> Vec<Vec<String>> {
    fn walk(cmd: &clap::Command, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        for sub in cmd.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            prefix.push(sub.get_name().to_owned());
            out.push(prefix.clone());
            walk(sub, prefix, out);
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(&Cli::command(), &mut Vec::new(), &mut out);
    out
}

#[tokio::test]
async fn help_lists_every_command() {
    let help = ratchet(&["--help"]).await;
    assert_eq!(help.code, 0, "{}", help.stderr);
    let listed = listed_in_help(&help.stdout);
    for want in COMMANDS {
        assert!(
            listed.iter().any(|l| l == want),
            "`ratchet --help` does not list `{want}`:\n{}",
            help.stdout
        );
    }
    for got in &listed {
        assert!(
            COMMANDS.contains(&got.as_str()),
            "`ratchet --help` lists `{got}`, which the contract (COMMANDS, and the ratchet skill) does not know yet"
        );
    }
}

#[tokio::test]
async fn every_command_has_help_of_its_own() {
    for path in command_paths() {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let out = ratchet(&args).await;
        assert_eq!(
            out.code,
            0,
            "ratchet {} --help: {}",
            path.join(" "),
            out.stderr
        );
        assert!(
            out.stdout.contains("Usage:"),
            "ratchet {} --help has no usage line:\n{}",
            path.join(" "),
            out.stdout
        );
    }
}

fn assert_json(o: &Out, what: &str) {
    assert_eq!(o.code, 0, "{what}: {}", o.stderr);
    serde_json::from_str::<serde_json::Value>(&o.stdout)
        .unwrap_or_else(|e| panic!("{what}: stdout is not JSON ({e}):\n{}", o.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn json_output_parses() {
    let env = Env::start().await;
    for args in [
        vec!["status"],
        vec!["list"],
        vec!["show", "opt_demo_search"],
        vec!["candidates", "opt_demo_search"],
        vec!["history", "opt_demo_search"],
        vec!["spend", "opt_demo_search"],
        vec!["activity"],
        vec!["overview"],
        vec!["connectors"],
        vec!["env", "list"],
    ] {
        let mut full = args.clone();
        full.push("--json");
        assert_json(&env.ratchet(&full).await, &args.join(" "));
    }
    // the offline commands too
    assert_json(&ratchet(&["templates", "--json"]).await, "templates");
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_id_exits_1_with_a_hint() {
    let env = Env::start().await;
    let human = env.ratchet(&["show", "no-such-id"]).await;
    assert_eq!(human.code, 1, "{}", human.stdout);
    assert!(human.stderr.starts_with("ratchet: "), "{}", human.stderr);
    assert!(
        human.stderr.contains("hint: ") && human.stderr.contains("ratchet list"),
        "{}",
        human.stderr
    );

    let json = env.ratchet(&["show", "no-such-id", "--json"]).await;
    assert_eq!(json.code, 1);
    let body: serde_json::Value = serde_json::from_str(json.stderr.trim())
        .unwrap_or_else(|e| panic!("stderr is not JSON ({e}): {}", json.stderr));
    assert_eq!(body["error"]["code"], "not_found", "{body}");
    assert!(
        body["error"]["hint"]
            .as_str()
            .is_some_and(|h| h.contains("ratchet list")),
        "{body}"
    );
    env.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn commands_that_change_things_need_yes() {
    let env = Env::start().await;
    for args in [
        vec!["stop", "opt_demo_search"],
        vec!["pr", "opt_demo_search"],
        vec!["env", "end", "sbx-1"],
    ] {
        let out = env.ratchet(&args).await;
        assert_eq!(out.code, 2, "{args:?} ran without --yes: {}", out.stdout);
        assert!(out.stderr.contains("--yes"), "{args:?}: {}", out.stderr);
    }
    env.stop().await;
}

#[tokio::test]
async fn open_builds_the_link_the_app_handles() {
    let page = ratchet(&["open", "connectors", "--print"]).await;
    assert_eq!(page.code, 0, "{}", page.stderr);
    assert_eq!(page.stdout.trim(), "ratchet://open/connectors");

    let id = ratchet(&["open", "opt_demo_search", "--print", "--json"]).await;
    assert_eq!(id.json()["url"], "ratchet://open/opt_demo_search");
    assert_eq!(id.json()["opened"], false);

    let odd = ratchet(&["open", "a b", "--print"]).await;
    assert_eq!(odd.stdout.trim(), "ratchet://open/a%20b");

    let empty = ratchet(&["open", "  ", "--print"]).await;
    assert_eq!(empty.code, 2);
}

#[tokio::test]
async fn the_agent_skill_teaches_the_commands_agents_need() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.claude/skills/ratchet/SKILL.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    assert!(
        text.starts_with("---\nname: ratchet\n"),
        "the skill needs front matter"
    );
    for needed in [
        "--json",
        "--yes",
        "ratchet chat",
        "ratchet mcp",
        "ratchet open",
        "ratchet status",
        "ratchet list",
        "ratchet run",
        "ratchet stop",
        "ratchet pr",
    ] {
        assert!(
            text.contains(needed),
            "the ratchet skill does not mention `{needed}`"
        );
    }
}
