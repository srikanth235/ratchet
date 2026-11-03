//! `ratchet init --template <id>` (offline). Port of `packages/cli/src/commands/init.ts`.

use std::path::{Path, PathBuf};

use ratchet_core::config::{format_config_error, parse_config};
use ratchet_core::enums::{Direction, TemplateKind};
use ratchet_core::templates::{
    RenderTemplateOptions, TemplateFilter, TemplateInfo, get_template, list_templates,
    render_template, resolve_template_id,
};
use serde_json::json;

use super::emit_json;
use crate::cli::InitArgs;
use crate::io::Io;

fn describe(t: &TemplateInfo) -> String {
    if t.kind == TemplateKind::Demo {
        let example = t.example.as_deref().unwrap_or("");
        return [
            format!("{}  [demo]  {}", t.id, t.title),
            format!("    {}", t.description),
            format!("    try it: {example} (runs in simulation, nothing to set up)"),
        ]
        .join("\n");
    }
    [
        format!("{}  {}", t.id, t.title),
        format!("    {}", t.description),
        format!(
            "    measures {}; {}; machine {}",
            t.measures,
            t.holdout
                .as_ref()
                .map_or("no holdout".to_owned(), |h| format!("holdout: {h}")),
            t.machine
        ),
        format!(
            "    default: {} {}; writes {}",
            t.objective.direction,
            t.objective.metric,
            t.files.join(", ")
        ),
    ]
    .join("\n")
}

fn kind_filter(kind: TemplateKind) -> TemplateFilter {
    TemplateFilter {
        kind: Some(kind),
        category: None,
    }
}

/// `ratchet init --list-templates` (also `ratchet templates`).
pub fn list_all(json: bool, io: &dyn Io) -> i32 {
    let templates = list_templates(kind_filter(TemplateKind::Template));
    let demos = list_templates(kind_filter(TemplateKind::Demo));
    if json {
        let mut all = templates;
        all.extend(demos);
        emit_json(io, &all);
        return 0;
    }
    let groups = [
        ("Templates (ratchet init --template <id>)", templates),
        ("Try a demo", demos),
    ];
    let text = groups
        .iter()
        .map(|(title, items)| {
            format!(
                "{title}\n\n{}",
                items.iter().map(describe).collect::<Vec<_>>().join("\n\n")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    io.out(&(text + "\n"));
    0
}

/// Returns the process exit code: 0 written (or dry run), 1 refused or invalid.
pub fn run_init(args: &InitArgs, json: bool, io: &dyn Io) -> i32 {
    if args.list_templates || args.list_packs {
        return list_all(json, io);
    }
    let ids: Vec<String> = list_templates(kind_filter(TemplateKind::Template))
        .into_iter()
        .map(|t| t.id)
        .collect();
    let Some(requested) = args.template.as_deref().or(args.pack.as_deref()) else {
        io.err(&format!(
            "init: --template is required (one of {}; see --list-templates)\n",
            ids.join(", ")
        ));
        return 1;
    };
    let Some(info) = get_template(requested) else {
        io.err(&format!(
            "init: unknown template {} (available: {})\n",
            serde_json::to_string(requested).unwrap_or_default(),
            ids.join(", ")
        ));
        return 1;
    };
    if info.kind == TemplateKind::Demo {
        io.err(&format!(
            "init: {} is a demo, not a template: it runs in simulation from {}; there is nothing to write into a repository\n",
            info.id,
            info.example.as_deref().unwrap_or("examples/")
        ));
        return 1;
    }
    let direction = match args.direction.as_deref() {
        None => None,
        Some("minimize") => Some(Direction::Minimize),
        Some("maximize") => Some(Direction::Maximize),
        Some(other) => {
            io.err(&format!(
                "init: --direction must be \"minimize\" or \"maximize\", got {}\n",
                serde_json::to_string(other).unwrap_or_default()
            ));
            return 1;
        }
    };
    if let Some(v) = args.value_usd
        && (!v.is_finite() || v <= 0.0)
    {
        io.err("init: --value-usd must be a positive number\n");
        return 1;
    }

    let options = RenderTemplateOptions {
        metric: args.metric.clone().filter(|m| !m.trim().is_empty()),
        direction,
        value_usd: args.value_usd,
    };
    let rendered = match render_template(&info.id, &options) {
        Ok(r) => r,
        Err(e) => {
            io.err(&format!("init: {e}\n"));
            return 1;
        }
    };
    let files = &rendered.files;
    let generated = &files[0];
    if let Err(errors) = parse_config(&generated.content) {
        io.err("init: the generated ratchet.yaml is invalid (check the options):\n");
        for e in &errors {
            io.err(&format!("  - {}\n", format_config_error(e)));
        }
        return 1;
    }

    let root = absolute(&args.dir);
    let existing: Vec<&str> = files
        .iter()
        .filter(|f| root.join(&f.path).exists())
        .map(|f| f.path.as_str())
        .collect();
    if !existing.is_empty() && !args.force {
        io.err(&format!(
            "init: refusing to overwrite {} (use --force)\n",
            existing.join(", ")
        ));
        return 1;
    }

    let verb = if args.dry_run { "would write" } else { "wrote" };
    let alias = match (&args.pack, &args.template) {
        (Some(p), None) if resolve_template_id(p) != p => {
            format!(" (--pack {p} is now the {} template)", info.id)
        }
        _ => String::new(),
    };
    let lines: Vec<String> = files
        .iter()
        .map(|f| {
            format!(
                "  {verb} {}{}  {} lines",
                f.path,
                if existing.contains(&f.path.as_str()) {
                    " (overwritten)"
                } else {
                    ""
                },
                f.content.split('\n').count().saturating_sub(1)
            )
        })
        .collect();
    let o = &rendered.options;
    if !args.dry_run {
        for f in files {
            let target = root.join(&f.path);
            if let Some(dir) = target.parent()
                && let Err(e) = std::fs::create_dir_all(dir)
            {
                io.err(&format!("init: cannot create {}: {e}\n", dir.display()));
                return 1;
            }
            if let Err(e) = std::fs::write(&target, &f.content) {
                io.err(&format!("init: cannot write {}: {e}\n", target.display()));
                return 1;
            }
        }
    }
    if json {
        emit_json(
            io,
            &json!({
                "template": info.id,
                "dir": root.display().to_string(),
                "dryRun": args.dry_run,
                "options": {"metric": o.metric, "direction": o.direction, "valueUsd": o.value_usd},
                "files": files.iter().map(|f| json!({
                    "path": f.path,
                    "lines": f.content.split('\n').count().saturating_sub(1),
                    "overwritten": existing.contains(&f.path.as_str()),
                })).collect::<Vec<_>>(),
            }),
        );
        return 0;
    }
    if args.dry_run {
        io.out(&format!(
            "ratchet init --template {}{alias} (dry run, nothing written to {})\n{}\n\n--- ratchet.yaml ---\n{}",
            info.id,
            root.display(),
            lines.join("\n"),
            generated.content
        ));
        return 0;
    }
    io.out(&format!(
        "ratchet init --template {}{alias}: {} {}, ${} per {} per year (estimated)\n{}\n\nNext: replace the TODOs, then `ratchet validate` and `ratchet baseline`.\n",
        info.id,
        o.direction,
        o.metric,
        ratchet_core::numfmt::js_number_to_string(o.value_usd),
        info.objective.unit,
        lines.join("\n")
    ));
    0
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}
