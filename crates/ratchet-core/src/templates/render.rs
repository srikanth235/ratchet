//! Turn a template into the files to commit. Port of `packages/core/src/templates/render.ts`.

use serde::{Deserialize, Serialize};

use super::files::template_files;
use super::registry::{TemplateInfo, find_def, get_template, resolve_template_id};
use crate::config::guardrails::js_trim;
use crate::enums::{Direction, TemplateKind};
use crate::numfmt::js_number_to_string;

/// A file to be written into a repository. `path` is repo-relative POSIX.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateFile {
    pub path: String,
    pub content: String,
}

/// What the user can tune when a template is turned into files. All optional: the template's defaults apply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderTemplateOptions {
    /// Objective metric, a dotted path into the evaluator's JSON output.
    pub metric: Option<String>,
    pub direction: Option<Direction>,
    /// Placeholder USD per unit of improvement per year (`objective.value`).
    pub value_usd: Option<f64>,
}

/// The options after defaults, as written to ratchet.yaml.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTemplateOptions {
    pub metric: String,
    pub direction: Direction,
    pub value_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedTemplate {
    pub info: TemplateInfo,
    pub options: ResolvedTemplateOptions,
    /// `ratchet.yaml` first, then `RATCHET.md` and the evaluator files.
    pub files: Vec<TemplateFile>,
}

/// Why a template could not be rendered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("unknown template {}", quote(.0))]
    Unknown(String),
    /// Demos have no files: they point at a project under `examples/`.
    #[error("{id} is a demo (see {example}); it has no files to add")]
    Demo { id: String, example: String },
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// A YAML scalar: plain when it is safe, JSON-quoted (valid YAML) otherwise.
/// Plain means `/^[A-Za-z_][\w.-]*$/`.
fn scalar(s: &str) -> String {
    let mut chars = s.chars();
    let plain = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if plain { s.to_owned() } else { quote(s) }
}

const FALLBACK_TYPES: [&str; 3] = ["c7i.large", "c7i.xlarge", "m7i.large"];

fn pad(width: usize, used: usize) -> String {
    " ".repeat(width.saturating_sub(used).max(1))
}

fn yaml_for(info: &TemplateInfo, o: &ResolvedTemplateOptions) -> String {
    let def = find_def(&info.id);
    let mut guardrails: Vec<String> = Vec::new();
    if def.is_some_and(|d| d.test_command) {
        guardrails.push(
            "  - run: npm test                  # TODO: your test command; must exit 0".to_owned(),
        );
    }
    for (k, v) in def.map(|d| d.guardrails).unwrap_or(&[]) {
        guardrails.push(format!("  - {k}: {}", quote(v)));
    }
    let mut types: Vec<&str> = Vec::new();
    for t in std::iter::once(info.machine.as_str()).chain(FALLBACK_TYPES) {
        if !types.contains(&t) {
            types.push(t);
        }
    }
    let category = info.category.as_str();
    let holdout = match &info.holdout {
        Some(h) => format!(
            "  # holdout: s3://sealed/{}-holdout.json  # TODO: the {h}, never shown to the agents (the winner is re-checked on it)\n",
            info.id
        ),
        None => String::new(),
    };
    format!(
        "version: 0
pack: {category}{pad_pack}# template category (docs/packs/{category}.md)
objective:
  name: {metric}
  metric: {metric}      # key in the evaluator's JSON output (dotted path)
  direction: {direction}
  # min_gain, the number of repeats and the noise floor are calibrated from the baseline's noise. To pin the win
  # threshold yourself, add `min_gain: 0.03` (a fraction) here and `repeats: 5` under evaluator.
  value:                           # business case: what one unit of the metric is worth
    usd_per_unit: {value_usd}    # USD per {unit} per year. A placeholder: replace it with your own numbers
    period: year
    basis: estimated               # becomes `measured` once confirmed in production
    unit: {unit_scalar}
evaluator:
  template: {id}{pad_id}# fills in the command, kind and default guardrails; its files are protected automatically
{holdout}environment:
  machine: {machine}{pad_machine}# bench hardware (pinned, dedicated host)
guardrails:
{guardrails}
protected:                         # added to what is protected automatically (ratchet.yaml, the evaluator, lockfiles)
  - package.json                   # defines the guardrail commands
agent:
  instructions: RATCHET.md         # brief injected into every session
  allowed_instance_types: [{types}]
",
        pad_pack = pad(27, category.chars().count()),
        metric = scalar(&o.metric),
        direction = o.direction,
        value_usd = js_number_to_string(o.value_usd),
        unit = info.objective.unit,
        unit_scalar = scalar(&info.objective.unit),
        id = info.id,
        pad_id = pad(29, info.id.chars().count()),
        machine = info.machine,
        pad_machine = pad(21, info.machine.chars().count()),
        guardrails = guardrails.join("\n"),
        types = types.join(", "),
    )
}

/// Turn a template into the files to commit: `ratchet.yaml` (the fragment, with the template's objective), the
/// `RATCHET.md` brief and the evaluator's real files. A legacy `--pack` name is accepted. Fails for an unknown
/// id and for a demo (demos have no files: they point at a project under `examples/`).
pub fn render_template(
    id_or_alias: &str,
    options: &RenderTemplateOptions,
) -> Result<RenderedTemplate, TemplateError> {
    let info =
        get_template(id_or_alias).ok_or_else(|| TemplateError::Unknown(id_or_alias.to_owned()))?;
    if info.kind == TemplateKind::Demo {
        return Err(TemplateError::Demo {
            id: info.id.clone(),
            example: info.example.clone().unwrap_or_default(),
        });
    }
    let o = ResolvedTemplateOptions {
        metric: options
            .metric
            .as_deref()
            .map(js_trim)
            .filter(|m| !m.is_empty())
            .map_or_else(|| info.objective.metric.clone(), str::to_owned),
        direction: options.direction.unwrap_or(info.objective.direction),
        value_usd: options.value_usd.unwrap_or(info.default_value_usd),
    };
    let fill = |text: &str| {
        text.replace("{{metric}}", &o.metric)
            .replace("{{direction}}", o.direction.as_str())
            .replace(
                "{{verb}}",
                if o.direction == Direction::Minimize {
                    "lower"
                } else {
                    "raise"
                },
            )
    };
    let mut files = vec![TemplateFile {
        path: "ratchet.yaml".into(),
        content: yaml_for(&info, &o),
    }];
    for (path, content) in template_files(resolve_template_id(id_or_alias)) {
        let content = if path == "RATCHET.md" {
            fill(content)
        } else {
            content.to_owned()
        };
        files.push(TemplateFile { path, content });
    }
    Ok(RenderedTemplate {
        info,
        options: o,
        files,
    })
}

/// The repo paths a template adds, `ratchet.yaml` included (demos and unknown ids: none).
pub fn template_file_paths(id_or_alias: &str) -> Vec<String> {
    let id = resolve_template_id(id_or_alias);
    match find_def(id) {
        Some(d) if d.kind == TemplateKind::Template => std::iter::once("ratchet.yaml".to_owned())
            .chain(template_files(id).into_iter().map(|(p, _)| p))
            .collect(),
        _ => Vec::new(),
    }
}
