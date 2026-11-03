//! The pull request that adds a template to a repository. Port of `packages/core/src/templates/propose.ts`.

use super::registry::{TemplateInfo, find_def};
use super::render::{RenderTemplateOptions, TemplateError, TemplateFile, render_template};

/// Everything needed to commit a template to a repository and open the PR. Pure data: no git, no network.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateProposal {
    pub template_id: String,
    pub info: TemplateInfo,
    /// Suggested branch name (the caller may add a suffix when it already exists).
    pub branch: String,
    pub commit_message: String,
    pub title: String,
    /// Markdown: what was added, what to fill in, how to run a baseline.
    pub body: String,
    pub files: Vec<TemplateFile>,
}

fn purpose(path: &str, info: &TemplateInfo) -> String {
    if path == "ratchet.yaml" {
        return "the optimization config: objective, evaluator template, guardrails".into();
    }
    if path == "RATCHET.md" {
        return "the brief every agent session starts from".into();
    }
    if path.starts_with("bench/") || path.starts_with("eval/") || path.starts_with("data/") {
        return if path.ends_with(".json") || path.ends_with(".jsonl") {
            "evaluator input you tune (protected)".into()
        } else {
            "the trusted evaluator (protected)".into()
        };
    }
    if path.starts_with("src/") {
        let whose = if info.category == crate::enums::TemplateCategory::Forecasting {
            "your forecaster"
        } else {
            "replace with yours"
        };
        return format!("the placeholder code the agents will improve ({whose})");
    }
    "template file".into()
}

/// Describe the PR that adds `template_id` to a repository: branch, commit message, title and a body that says
/// what was added, what still needs filling in, and how to run a baseline. Fails for an unknown template or a demo.
pub fn build_template_proposal(
    template_id: &str,
    options: &RenderTemplateOptions,
) -> Result<TemplateProposal, TemplateError> {
    let rendered = render_template(template_id, options)?;
    let (info, files, o) = (rendered.info, rendered.files, rendered.options);
    let rows: Vec<String> = files
        .iter()
        .map(|f| format!("| `{}` | {} |", f.path, purpose(&f.path, &info)))
        .collect();
    let rows = rows.join("\n");
    let evaluator_cmd = find_def(&info.id).and_then(|d| d.run).unwrap_or("");
    let holdout = match &info.holdout {
        Some(h) => {
            format!("\n**Holdout:** every win is re-checked on a {h} that the agents never see.")
        }
        None => String::new(),
    };
    let tunable: Vec<String> = files
        .iter()
        .filter(|f| {
            (f.path.ends_with(".config") || f.path.ends_with(".json") || f.path.ends_with(".jsonl"))
                && !f.path.ends_with("package.json")
        })
        .map(|f| format!("`{}`", f.path))
        .collect();
    let tunable = if tunable.is_empty() {
        "see the evaluator file header".to_owned()
    } else {
        tunable.join(", ")
    };

    let body = format!(
        "## Add the Ratchet \"{title}\" template

This adds what Ratchet needs to {description}: a trusted {kind} that agents cannot edit, the config and a brief. It only adds files; it does not change your code.

**Objective:** {direction} `{metric}` ({unit}).
**Measures:** {measures}.{holdout}
**Bench machine:** `{machine}`.

### What was added

| File | Purpose |
|---|---|
{rows}

`ratchet.yaml`, the evaluator files above, lockfiles and install config are protected automatically: a candidate that edits them is blocked.

### Before you start an optimization

- Fill in the `TODO`s: the evaluator's config ({tunable}), the guardrail test command in `ratchet.yaml`, and the brief in `RATCHET.md`.
- Replace the placeholder metric value (`objective.value`) with your own numbers if you want a dollar figure on the result.

### Run a baseline

```sh
{evaluator_cmd}          # the evaluator on its own: prints the JSON that contains `{metric}`
ratchet validate         # checks ratchet.yaml
ratchet baseline         # runs the evaluator several times: prints the noise and the gain a win must beat
```

In the Ratchet app: New optimization, pick this repository, and the Review step runs the baseline and shows the
noise and the gain a win has to beat.
",
        title = info.title,
        description = info.description,
        kind = info.evaluator_kind,
        direction = o.direction,
        metric = o.metric,
        unit = info.objective.unit,
        measures = info.measures,
        machine = info.machine,
    );
    Ok(TemplateProposal {
        template_id: info.id.clone(),
        branch: format!("ratchet/template-{}", info.id),
        commit_message: format!(
            "Add Ratchet {} template\n\nAdds ratchet.yaml, RATCHET.md and the {} evaluator files.",
            info.title, info.id
        ),
        title: format!("Add Ratchet \"{}\" template", info.title),
        info,
        body,
        files,
    })
}
