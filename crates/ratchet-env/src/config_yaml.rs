//! Setting the `environment:` keys of a ratchet.yaml text (`config-yaml.ts`).
//!
//! DELIBERATE DIFFERENCE: the TypeScript uses the `yaml` Document API (comments survive, but the text is
//! re-serialized, so quoting and wrapping may change). `serde_norway` cannot keep comments, so this is a TEXT-LEVEL
//! edit of the `environment:` block: every byte outside the block is kept, the block's other keys and comments are
//! kept, and only the patched keys are replaced or appended. The parsed meaning is the same.

use ratchet_core::enums::EnvironmentScope;
use serde_norway::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentConfigPatch {
    pub scope: EnvironmentScope,
    pub machine: Option<String>,
    pub image: Option<String>,
    pub region: Option<String>,
}

impl EnvironmentConfigPatch {
    pub fn scope(scope: EnvironmentScope) -> Self {
        Self {
            scope,
            machine: None,
            image: None,
            region: None,
        }
    }

    /// `(key, value)` in the order the TypeScript applies them.
    fn entries(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![("scope", self.scope.to_string())];
        for (k, x) in [
            ("machine", &self.machine),
            ("image", &self.image),
            ("region", &self.region),
        ] {
            if let Some(x) = x {
                v.push((k, x.clone()));
            }
        }
        v
    }
}

fn scalar(s: &str) -> String {
    serde_norway::to_string(&Value::String(s.to_owned()))
        .map(|t| t.trim_end().to_owned())
        .unwrap_or_else(|_| format!("{s:?}"))
}

fn parse_error(text: &str) -> Option<String> {
    match serde_norway::from_str::<Value>(text) {
        Ok(_) => None,
        Err(e) => {
            let m = e.to_string();
            Some(m.lines().next().unwrap_or("").to_owned())
        }
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

fn line_body(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

fn line_end(line: &str) -> &str {
    &line[line_body(line).len()..]
}

/// Set the `environment:` keys of a ratchet.yaml text and nothing else (comments, key order and every other block
/// are kept). `Err` carries the message for text that is not valid YAML.
pub fn apply_environment_to_config_yaml(
    text: &str,
    patch: &EnvironmentConfigPatch,
) -> Result<String, String> {
    if let Some(m) = parse_error(text) {
        return Err(format!("ratchet.yaml is not valid YAML: {m}"));
    }
    let doc: Value = serde_norway::from_str(text).unwrap_or(Value::Null);
    if !matches!(doc, Value::Null | Value::Mapping(_)) {
        return Err("ratchet.yaml is not valid YAML: the document is not a mapping".into());
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let entries = patch.entries();

    // the top-level `environment:` key
    let key_at = lines.iter().position(|l| {
        let b = line_body(l);
        b.strip_prefix("environment")
            .is_some_and(|r| r.trim_start_matches([' ', '\t']).starts_with(':'))
    });
    let Some(k) = key_at else {
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(eol);
        }
        out.push_str(&format!("environment:{eol}"));
        for (key, val) in &entries {
            out.push_str(&format!("  {key}: {}{eol}", scalar(val)));
        }
        return Ok(out);
    };

    // the block: lines after the key that are blank or indented (a column-0 line, comment or key, ends it)
    let mut end = k + 1;
    while end < lines.len() {
        let b = line_body(lines[end]);
        if b.trim().is_empty() || b.starts_with([' ', '\t']) {
            end += 1;
        } else {
            break;
        }
    }
    // trailing blank / comment-only lines are not part of the block's content
    let mut content_end = end;
    while content_end > k + 1 && is_blank_or_comment(lines[content_end - 1]) {
        content_end -= 1;
    }

    let key_line = line_body(lines[k]);
    let after_colon = key_line
        .split_once(':')
        .map(|(_, r)| r.trim())
        .unwrap_or("");
    let inline_value = !(after_colon.is_empty() || after_colon.starts_with('#'));

    let mut out = String::new();
    for l in &lines[..k] {
        out.push_str(l);
    }
    if inline_value {
        // flow style (`environment: {image: x}`) or an explicit null: rewrite the whole block in block style
        let current = match &doc {
            Value::Mapping(m) => m.get("environment").cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        };
        let mut map = match current {
            Value::Mapping(m) => m,
            _ => serde_norway::Mapping::new(),
        };
        for (key, val) in &entries {
            map.insert(Value::String((*key).into()), Value::String(val.clone()));
        }
        let body = serde_norway::to_string(&Value::Mapping(map)).map_err(|e| e.to_string())?;
        out.push_str(&format!("environment:{eol}"));
        for l in body.lines() {
            out.push_str(&format!("  {l}{eol}"));
        }
        for l in &lines[end..] {
            out.push_str(l);
        }
        return Ok(out);
    }

    out.push_str(lines[k]);
    let mut body: Vec<String> = lines[k + 1..content_end]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    // make sure the key line ends a line before anything is appended
    if !lines[k].ends_with('\n') {
        out.push_str(eol);
    }
    let child_indent = body
        .iter()
        .find(|l| !is_blank_or_comment(l))
        .map(|l| indent_of(l))
        .unwrap_or(2);
    let pad = " ".repeat(child_indent);
    for (key, val) in &entries {
        let new_line_for = |ending: &str| format!("{pad}{key}: {}{ending}", scalar(val));
        let is_key_line = |l: &str| {
            let b = line_body(l);
            indent_of(b) == child_indent
                && b[child_indent..]
                    .strip_prefix(*key)
                    .is_some_and(|r| r.trim_start_matches([' ', '\t']).starts_with(':'))
        };
        if let Some(pos) = body.iter().position(|l| is_key_line(l)) {
            // drop the old value's continuation lines (deeper-indented), then replace the key line
            let mut stop = pos + 1;
            while stop < body.len()
                && !is_blank_or_comment(&body[stop])
                && indent_of(line_body(&body[stop])) > child_indent
            {
                stop += 1;
            }
            let ending = line_end(&body[pos]).to_owned();
            let ending = if ending.is_empty() {
                eol.to_owned()
            } else {
                ending
            };
            // keep a trailing comment when the old value was a plain scalar
            let old = line_body(&body[pos]);
            let comment = old
                .split_once(':')
                .map(|(_, v)| v)
                .filter(|v| !v.contains(['"', '\'']))
                .and_then(|v| v.find(" #").map(|i| v[i..].to_owned()))
                .unwrap_or_default();
            let line = new_line_for(&ending);
            let line = if comment.is_empty() {
                line
            } else {
                format!("{}{}{}", line_body(&line), comment, ending)
            };
            body.splice(pos..stop, [line]);
        } else {
            // append after the last content line
            let at = body
                .iter()
                .rposition(|l| !is_blank_or_comment(l))
                .map(|i| i + 1)
                .unwrap_or(0);
            if at > 0 && !body[at - 1].ends_with('\n') {
                let fixed = format!("{}{eol}", body[at - 1]);
                body[at - 1] = fixed;
            }
            body.insert(at, new_line_for(eol));
        }
    }
    for l in &body {
        out.push_str(l);
    }
    // blank / comment-only lines that closed the block, then the rest of the document
    for l in &lines[content_end..] {
        out.push_str(l);
    }
    Ok(out)
}

/// `environment.scope` of a ratchet.yaml text, or `None` (also for text that is not valid YAML).
pub fn read_environment_scope(text: &str) -> Option<EnvironmentScope> {
    let doc: Value = serde_norway::from_str(text).ok()?;
    let s = doc.get("environment")?.get("scope")?.as_str()?;
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "# my config
version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
evaluator:
  template: http-latency   # trusted evaluation
protected:
  - tests/**
";

    fn patch(scope: EnvironmentScope) -> EnvironmentConfigPatch {
        EnvironmentConfigPatch::scope(scope)
    }

    // "applyEnvironmentToConfigYaml sets only environment keys and keeps comments, order and the rest"
    #[test]
    fn sets_only_environment_keys_and_keeps_the_rest() {
        let mut p = patch(EnvironmentScope::Single);
        p.machine = Some("c7i.xlarge".into());
        p.image = Some("infra/ratchet/Dockerfile".into());
        let out = apply_environment_to_config_yaml(CONFIG, &p).unwrap();
        assert!(out.starts_with(CONFIG), "{out}");
        assert!(
            out.contains("# my config")
                && out.contains("# trusted evaluation")
                && out.contains("tests/**")
        );
        assert_eq!(read_environment_scope(&out), Some(EnvironmentScope::Single));
        let again = apply_environment_to_config_yaml(&out, &patch(EnvironmentScope::None)).unwrap();
        assert!(again.contains("machine: c7i.xlarge"));
        assert_eq!(read_environment_scope(&again), Some(EnvironmentScope::None));
        assert_eq!(read_environment_scope("not: [valid"), None);
        assert_eq!(read_environment_scope(CONFIG), None);
    }

    fn env_of(text: &str) -> Value {
        let v: Value = serde_norway::from_str(text).unwrap();
        v.get("environment").cloned().unwrap_or(Value::Null)
    }

    #[test]
    fn edits_blocks_in_every_shape() {
        let p = patch(EnvironmentScope::Full);
        // present with other keys, scope in place
        let a = apply_environment_to_config_yaml(
            "a: 1\nenvironment:\n  region: us-east-1  # r\n  scope: none # s\n  network:\n    - x.example.com\nz: 2\n",
            &p,
        )
        .unwrap();
        assert_eq!(
            a,
            "a: 1\nenvironment:\n  region: us-east-1  # r\n  scope: full # s\n  network:\n    - x.example.com\nz: 2\n"
        );
        // appended to a block, before trailing comments and the next key
        let b = apply_environment_to_config_yaml(
            "environment:\n  region: us-east-1\n\n# next\nz: 2\n",
            &p,
        )
        .unwrap();
        assert_eq!(
            b,
            "environment:\n  region: us-east-1\n  scope: full\n\n# next\nz: 2\n"
        );
        // flow style is rewritten
        let c = apply_environment_to_config_yaml("environment: {image: x}\nz: 2\n", &p).unwrap();
        assert_eq!(env_of(&c).get("image").and_then(Value::as_str), Some("x"));
        assert_eq!(
            env_of(&c).get("scope").and_then(Value::as_str),
            Some("full")
        );
        assert!(c.ends_with("z: 2\n"));
        // null / empty
        let d = apply_environment_to_config_yaml("environment:\nz: 2\n", &p).unwrap();
        assert_eq!(d, "environment:\n  scope: full\nz: 2\n");
        let e = apply_environment_to_config_yaml("environment: null\nz: 2\n", &p).unwrap();
        assert_eq!(
            env_of(&e).get("scope").and_then(Value::as_str),
            Some("full")
        );
        // no trailing newline, CRLF, empty text
        assert_eq!(
            apply_environment_to_config_yaml("a: 1", &p).unwrap(),
            "a: 1\nenvironment:\n  scope: full\n"
        );
        assert_eq!(
            apply_environment_to_config_yaml("a: 1\r\nb: 2\r\n", &p).unwrap(),
            "a: 1\r\nb: 2\r\nenvironment:\r\n  scope: full\r\n"
        );
        assert_eq!(
            apply_environment_to_config_yaml("", &p).unwrap(),
            "environment:\n  scope: full\n"
        );
        // a longer scalar value and nested continuation lines are replaced
        let mut q = patch(EnvironmentScope::Single);
        q.image = Some("infra/ratchet/Dockerfile".into());
        let f = apply_environment_to_config_yaml(
            "environment:\n  image:\n    - a\n  scope: none\n",
            &q,
        )
        .unwrap();
        assert_eq!(
            f,
            "environment:\n  image: infra/ratchet/Dockerfile\n  scope: single\n"
        );
        // invalid yaml
        assert!(
            apply_environment_to_config_yaml("a: [", &p)
                .unwrap_err()
                .starts_with("ratchet.yaml is not valid YAML: ")
        );
    }
}
