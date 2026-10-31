//! A small, tolerant reader for the HCL subset a drafted environment is written in (`hcl.ts`).
//!
//! It exists so the validator can look at what a setup agent wrote WITHOUT running anything: blocks
//! (`resource "t" "n" { ... }`), attributes, nested objects / lists, strings (with `${}` interpolation flagged),
//! heredocs and comments. It is deliberately not a full HCL parser. Whatever it cannot read becomes an
//! [`HclValue::Expr`] (raw text) and the validator treats an expression where it needs a literal as "not pinned".
//! The authoritative check is still `tofu validate` / `tofu plan`.
//!
//! The scanner works over `char`s (the TypeScript over UTF-16 units; identical for everything but astral characters),
//! so line numbers and `raw` slices match.

#[derive(Debug, Clone, PartialEq)]
pub enum HclValue {
    String {
        value: String,
        dynamic: bool,
    },
    Number(f64),
    Bool(bool),
    Null,
    /// Entries in source order (a repeated key keeps its first position and its last value, like a JS object).
    Object(Vec<(String, HclValue)>),
    List(Vec<HclValue>),
    /// Raw text of whatever the reader could not read as a literal.
    Expr(String),
}

impl HclValue {
    /// Entry `key` of an object value.
    pub fn entry(&self, key: &str) -> Option<&HclValue> {
        match self {
            HclValue::Object(e) => e.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HclBlock {
    /// `resource`, `module`, `data`, `variable`, `output`, `provider`, `terraform`, `locals`, `lifecycle`, ...
    pub block_type: String,
    pub labels: Vec<String>,
    /// Attributes in source order.
    pub attrs: Vec<(String, HclValue)>,
    pub blocks: Vec<HclBlock>,
    /// The block's source text (header and body).
    pub raw: String,
    /// 1-based line of the block header.
    pub line: usize,
}

impl HclBlock {
    pub fn attr(&self, name: &str) -> Option<&HclValue> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HclFile {
    /// Top-level blocks. Top-level attributes do not exist in a module.
    pub blocks: Vec<HclBlock>,
    /// Why the file could not be read completely (unbalanced braces, unterminated string). Empty when fine.
    pub errors: Vec<String>,
}

fn is_space(c: Option<char>) -> bool {
    matches!(c, Some(' ' | '\t' | '\r'))
}
fn is_ident_start(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}
fn is_ident(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn at(src: &[char], i: usize) -> Option<char> {
    src.get(i).copied()
}

fn slice(src: &[char], a: usize, b: usize) -> String {
    let b = b.min(src.len());
    let a = a.min(b);
    src[a..b].iter().collect()
}

fn index_of(src: &[char], c: char, from: usize) -> Option<usize> {
    (from..src.len()).find(|&i| src[i] == c)
}

/// JavaScript `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// Replace comments with spaces (keeping newlines and string contents), so offsets and line numbers stay valid.
pub fn strip_comments(source: &str) -> String {
    let src: Vec<char> = source.chars().collect();
    strip_comments_chars(&src).into_iter().collect()
}

fn strip_comments_chars(src: &[char]) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(src.len());
    let n = src.len();
    let mut i = 0;
    while i < n {
        let c = src[i];
        let c2 = at(src, i + 1);
        if c == '"' {
            let end = skip_string(src, i);
            out.extend_from_slice(&src[i..end.min(n)]);
            i = end;
        } else if c == '<' && c2 == Some('<') && heredoc_start(src, i).is_some() {
            let end = skip_heredoc(src, i);
            out.extend_from_slice(&src[i..end.min(n)]);
            i = end;
        } else if c == '#' || (c == '/' && c2 == Some('/')) {
            while i < n && src[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && c2 == Some('*') {
            let close = (i + 2..n).find(|&k| src[k] == '*' && at(src, k + 1) == Some('/'));
            let end = close.map_or(n, |k| k + 2);
            for &ch in &src[i..end] {
                out.push(if ch == '\n' { '\n' } else { ' ' });
            }
            i = end;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Index just past the closing quote of the string starting at `i` (handles `\"` and nested `${ "..." }`).
fn skip_string(src: &[char], i: usize) -> usize {
    let mut k = i + 1;
    while k < src.len() {
        let c = src[k];
        if c == '\\' {
            k += 2;
        } else if c == '"' {
            return k + 1;
        } else if c == '$' && at(src, k + 1) == Some('{') {
            k = skip_interpolation(src, k + 2);
        } else if c == '\n' {
            return k; // unterminated: stop at the line end
        } else {
            k += 1;
        }
    }
    k
}

fn skip_interpolation(src: &[char], from: usize) -> usize {
    let mut depth = 1;
    let mut k = from;
    while k < src.len() && depth > 0 {
        let c = src[k];
        if c == '"' {
            k = skip_string(src, k);
        } else {
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
            }
            k += 1;
        }
    }
    k
}

/// `^<<-?([A-Za-z_][A-Za-z0-9_]*)[ \t]*\r?\n` on the 200 characters at `i`: `(tag, length of the match)`.
fn heredoc_start(src: &[char], i: usize) -> Option<(String, usize)> {
    let end = (i + 200).min(src.len());
    let w = &src[i.min(end)..end];
    if w.len() < 2 || w[0] != '<' || w[1] != '<' {
        return None;
    }
    let mut k = 2;
    if w.get(k) == Some(&'-') {
        k += 1;
    }
    let s = k;
    if !w
        .get(k)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
    {
        return None;
    }
    k += 1;
    while w
        .get(k)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
    {
        k += 1;
    }
    let tag: String = w[s..k].iter().collect();
    while matches!(w.get(k), Some(' ' | '\t')) {
        k += 1;
    }
    if w.get(k) == Some(&'\r') {
        k += 1;
    }
    if w.get(k) == Some(&'\n') {
        Some((tag, k + 1))
    } else {
        None
    }
}

fn skip_heredoc(src: &[char], i: usize) -> usize {
    let Some((tag, len)) = heredoc_start(src, i) else {
        return i + 2;
    };
    let mut k = i + len;
    while k < src.len() {
        let eol = index_of(src, '\n', k);
        let line = slice(src, k, eol.unwrap_or(src.len()));
        if js_trim(&line) == tag {
            return match eol {
                None => src.len(),
                Some(_) => k + line.chars().count(),
            };
        }
        match eol {
            None => return src.len(),
            Some(e) => k = e + 1,
        }
    }
    src.len()
}

/// Index of the brace matching the `{` / `[` at `open`, or `None`. Strings, heredocs and (already stripped)
/// comments are skipped.
fn matching(src: &[char], open: usize) -> Option<usize> {
    let mut stack: Vec<char> = vec![];
    let mut k = open;
    while k < src.len() {
        let c = src[k];
        if c == '"' {
            k = skip_string(src, k);
            continue;
        }
        if c == '<' && at(src, k + 1) == Some('<') && heredoc_start(src, k).is_some() {
            k = skip_heredoc(src, k);
            continue;
        }
        if matches!(c, '{' | '[' | '(') {
            stack.push(c);
        } else if matches!(c, '}' | ']' | ')') {
            let want = match c {
                '}' => '{',
                ']' => '[',
                _ => '(',
            };
            if stack.pop() != Some(want) {
                return None;
            }
            if stack.is_empty() {
                return Some(k);
            }
        }
        k += 1;
    }
    None
}

fn line_of(src: &[char], offset: usize) -> usize {
    1 + src[..offset.min(src.len())]
        .iter()
        .filter(|&&c| c == '\n')
        .count()
}

/// Parse a whole .tf file into its top-level blocks.
pub fn parse_hcl(source: &str) -> HclFile {
    let original: Vec<char> = source.chars().collect();
    let src = strip_comments_chars(&original);
    let mut errors = vec![];
    let body = parse_body(&src, 0, src.len(), &original, &mut errors);
    HclFile {
        blocks: body.blocks,
        errors,
    }
}

struct Body {
    attrs: Vec<(String, HclValue)>,
    blocks: Vec<HclBlock>,
}

fn set_attr(attrs: &mut Vec<(String, HclValue)>, key: String, v: HclValue) {
    if let Some(slot) = attrs.iter_mut().find(|(k, _)| *k == key) {
        slot.1 = v;
    } else {
        attrs.push((key, v));
    }
}

fn json_char(c: char) -> String {
    serde_json::to_string(&c.to_string()).unwrap_or_default()
}

fn parse_body(
    src: &[char],
    from: usize,
    to: usize,
    original: &[char],
    errors: &mut Vec<String>,
) -> Body {
    let mut attrs: Vec<(String, HclValue)> = vec![];
    let mut blocks: Vec<HclBlock> = vec![];
    let mut i = from;
    while i < to {
        while i < to
            && (is_space(at(src, i)) || at(src, i) == Some('\n') || at(src, i) == Some(','))
        {
            i += 1;
        }
        if i >= to {
            break;
        }
        if !is_ident_start(at(src, i)) {
            errors.push(format!(
                "unexpected {} on line {}",
                json_char(src[i]),
                line_of(original, i)
            ));
            // resynchronise on the next line
            i = match index_of(src, '\n', i) {
                Some(nl) if nl < to => nl + 1,
                _ => to,
            };
            continue;
        }
        let start = i;
        while i < to && is_ident(at(src, i)) {
            i += 1;
        }
        let word = slice(src, start, i);
        while i < to && is_space(at(src, i)) {
            i += 1;
        }
        if at(src, i) == Some('=') && at(src, i + 1) != Some('=') {
            i += 1;
            let Some(end) = value_end(src, i, to) else {
                errors.push(format!(
                    "unbalanced value for \"{word}\" on line {}",
                    line_of(original, start)
                ));
                break;
            };
            let v = parse_value(src, i, end, original, errors);
            set_attr(&mut attrs, word, v);
            i = end;
            continue;
        }
        if at(src, i) == Some(':') && at(src, i + 1) != Some(':') {
            // object-constructor style `key: value`
            i += 1;
            let Some(end) = value_end(src, i, to) else {
                break;
            };
            let v = parse_value(src, i, end, original, errors);
            set_attr(&mut attrs, word, v);
            i = end;
            continue;
        }
        // a block: labels (quoted or bare), then `{`
        let mut labels: Vec<String> = vec![];
        loop {
            while i < to && is_space(at(src, i)) {
                i += 1;
            }
            if at(src, i) == Some('"') {
                let end = skip_string(src, i);
                labels.push(slice(src, i + 1, end.saturating_sub(1)));
                i = end;
            } else if is_ident_start(at(src, i)) {
                let s = i;
                while i < to && is_ident(at(src, i)) {
                    i += 1;
                }
                labels.push(slice(src, s, i));
            } else {
                break;
            }
        }
        if at(src, i) != Some('{') {
            errors.push(format!(
                "expected \"{{\" after \"{word}\" on line {}",
                line_of(original, start)
            ));
            i = match index_of(src, '\n', i) {
                Some(nl) if nl < to => nl + 1,
                _ => to,
            };
            continue;
        }
        let close = match matching(src, i) {
            Some(c) if c < to => c,
            _ => {
                errors.push(format!(
                    "block \"{word}\" starting on line {} is never closed",
                    line_of(original, start)
                ));
                break;
            }
        };
        let inner = parse_body(src, i + 1, close, original, errors);
        blocks.push(HclBlock {
            block_type: word,
            labels,
            attrs: inner.attrs,
            blocks: inner.blocks,
            raw: slice(original, start, close + 1),
            line: line_of(original, start),
        });
        i = close + 1;
    }
    Body { attrs, blocks }
}

/// End offset of the expression starting at `from`: the end of the line, or of the bracketed group that spans lines.
fn value_end(src: &[char], from: usize, to: usize) -> Option<usize> {
    let mut k = from;
    while k < to {
        let c = src[k];
        if c == '"' {
            k = skip_string(src, k);
            continue;
        }
        if c == '<' && at(src, k + 1) == Some('<') && heredoc_start(src, k).is_some() {
            return Some(skip_heredoc(src, k).min(to));
        }
        if matches!(c, '{' | '[' | '(') {
            let close = matching(src, k)?;
            k = close + 1;
            continue;
        }
        if c == ',' {
            return Some(k); // inside an object constructor: `{ a = 1, b = 2 }`
        }
        if c == '\n' {
            // a trailing operator / comma continues the expression on the next line
            let mut b = k as isize - 1;
            while b >= from as isize && is_space(at(src, b as usize)) {
                b -= 1;
            }
            if b >= from as isize {
                let t = src[b as usize];
                if "+-*/?:&|,=<>!".contains(t) && t != ',' {
                    k += 1;
                    continue;
                }
            }
            return Some(k);
        }
        k += 1;
    }
    Some(to)
}

fn has_interp(s: &str) -> bool {
    s.contains("${") || s.contains("%{")
}

fn parse_value(
    src: &[char],
    from: usize,
    to: usize,
    original: &[char],
    errors: &mut Vec<String>,
) -> HclValue {
    let mut a = from;
    let mut b = to;
    while a < b && (is_space(at(src, a)) || at(src, a) == Some('\n')) {
        a += 1;
    }
    while b > a && (is_space(at(src, b - 1)) || matches!(at(src, b - 1), Some('\n' | ','))) {
        b -= 1;
    }
    let text = slice(src, a, b);
    if text.is_empty() {
        return HclValue::Expr(String::new());
    }
    if text.starts_with('"') {
        let end = skip_string(src, a);
        if end == b {
            let inner = slice(src, a + 1, b.saturating_sub(1));
            let dynamic = has_interp(&inner);
            return HclValue::String {
                value: if dynamic { inner } else { unescape(&inner) },
                dynamic,
            };
        }
        return HclValue::Expr(text);
    }
    if text.starts_with("<<")
        && let Some((_, mlen)) = heredoc_start(src, a)
    {
        let body: String = text.chars().skip(mlen).collect();
        let mut lines: Vec<&str> = body.split('\n').collect();
        lines.pop(); // closing tag
        let value = lines.join("\n");
        return HclValue::String {
            dynamic: has_interp(&value),
            value,
        };
    }
    if text.starts_with('{') && matching(src, a) == Some(b - 1) {
        return HclValue::Object(parse_body(src, a + 1, b - 1, original, errors).attrs);
    }
    if text.starts_with('[') && matching(src, a) == Some(b - 1) {
        let items = split_list(src, a + 1, b - 1)
            .into_iter()
            .map(|(s, e)| parse_value(src, s, e, original, errors))
            .collect();
        return HclValue::List(items);
    }
    if text == "true" || text == "false" {
        return HclValue::Bool(text == "true");
    }
    if text == "null" {
        return HclValue::Null;
    }
    if is_number(&text) {
        return HclValue::Number(text.parse().unwrap_or(f64::NAN));
    }
    HclValue::Expr(text)
}

/// `/^-?\d+(\.\d+)?$/`
fn is_number(t: &str) -> bool {
    let t = t.strip_prefix('-').unwrap_or(t);
    let (int, frac) = match t.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (t, None),
    };
    !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

fn split_list(src: &[char], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut start = from;
    let mut k = from;
    while k < to {
        let c = src[k];
        if c == '"' {
            k = skip_string(src, k);
            continue;
        }
        if matches!(c, '{' | '[' | '(') {
            k = matching(src, k).map_or(to, |c| c + 1);
            continue;
        }
        if c == ',' {
            out.push((start, k));
            start = k + 1;
        }
        k += 1;
    }
    if !js_trim(&slice(src, start, to)).is_empty() {
        out.push((start, to));
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\'
            && let Some(&n) = it.peek()
            && matches!(n, '"' | '\\' | 'n' | 'r' | 't')
        {
            it.next();
            out.push(match n {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                o => o,
            });
        } else {
            out.push(c);
        }
    }
    out
}

// ---- helpers for the validator ----------------------------------------------------------------------------------------

/// The literal string of a value, or `None` when it is not a plain (non-interpolated) string.
pub fn literal_string(v: Option<&HclValue>) -> Option<String> {
    match v {
        Some(HclValue::String {
            value,
            dynamic: false,
        }) => Some(value.clone()),
        _ => None,
    }
}

pub fn literal_number(v: Option<&HclValue>) -> Option<f64> {
    match v {
        Some(HclValue::Number(n)) => Some(*n),
        _ => None,
    }
}

/// Every block (each one included) in a tree, depth first.
pub fn walk_blocks(blocks: &[HclBlock]) -> Vec<&HclBlock> {
    let mut out = vec![];
    for b in blocks {
        out.push(b);
        out.extend(walk_blocks(&b.blocks));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(v: &HclValue) -> Vec<&str> {
        match v {
            HclValue::Object(e) => e.iter().map(|(k, _)| k.as_str()).collect(),
            _ => panic!("not an object"),
        }
    }

    // "reads blocks, labels, attributes and nested blocks"
    #[test]
    fn reads_blocks_labels_attributes_and_nested_blocks() {
        let f = parse_hcl(
            r#"
      resource "aws_s3_object" "seed" {
        bucket = module.sandbox.artifacts_bucket
        key    = "env/seed.sql"
        count  = 2
        flag   = true
        nothing = null
        lifecycle {
          prevent_destroy = false
        }
      }
      variable "x" {}
    "#,
        );
        assert_eq!(f.errors, Vec::<String>::new());
        let heads: Vec<Vec<&str>> = f
            .blocks
            .iter()
            .map(|b| {
                let mut v = vec![b.block_type.as_str()];
                v.extend(b.labels.iter().map(String::as_str));
                v
            })
            .collect();
        assert_eq!(
            heads,
            vec![
                vec!["resource", "aws_s3_object", "seed"],
                vec!["variable", "x"]
            ]
        );
        let r = &f.blocks[0];
        assert_eq!(
            r.attr("bucket"),
            Some(&HclValue::Expr("module.sandbox.artifacts_bucket".into()))
        );
        assert_eq!(
            literal_string(r.attr("key")).as_deref(),
            Some("env/seed.sql")
        );
        assert_eq!(literal_number(r.attr("count")), Some(2.0));
        assert_eq!(r.attr("flag"), Some(&HclValue::Bool(true)));
        assert_eq!(r.attr("nothing"), Some(&HclValue::Null));
        assert_eq!(r.blocks[0].block_type, "lifecycle");
        assert_eq!(
            r.blocks[0].attr("prevent_destroy"),
            Some(&HclValue::Bool(false))
        );
    }

    // "reads nested objects with newline or comma separators, function calls and heredocs"
    #[test]
    fn reads_nested_objects_function_calls_and_heredocs() {
        let f = parse_hcl(
            r#"
module "sandbox" {
  source = "./ratchet-sandbox"
  machines = {
    api = { role = "app", instance_type = "c7i.large" }
    db = {
      role          = "database"
      user_data     = templatefile("${path.module}/db.sh", { bucket = var.ratchet.artifacts_bucket })
      detail        = <<-EOT
        hello # not a comment
        } stray brace in a heredoc
      EOT
    }
  }
}
"#,
        );
        assert_eq!(f.errors, Vec::<String>::new());
        let m = f.blocks[0].attr("machines").unwrap();
        assert_eq!(kv(m), vec!["api", "db"]);
        let api = m.entry("api").unwrap();
        assert_eq!(
            literal_string(api.entry("instance_type")).as_deref(),
            Some("c7i.large")
        );
        let db = m.entry("db").unwrap();
        assert!(matches!(db.entry("user_data"), Some(HclValue::Expr(_))));
        match db.entry("detail") {
            Some(HclValue::String { value, .. }) => {
                assert!(value.contains("stray brace in a heredoc"));
            }
            other => panic!("{other:?}"),
        }
    }

    // "ignores comments of every style but keeps strings that look like them"
    #[test]
    fn ignores_comments_but_keeps_strings_that_look_like_them() {
        let f = parse_hcl(
            r#"
# a comment with { a brace
// another one }
/* multi
   line { */
locals {
  url = "http://example.test/#not-a-comment"
  note = "a // b"
}
"#,
        );
        assert_eq!(f.errors, Vec::<String>::new());
        assert_eq!(
            literal_string(f.blocks[0].attr("url")).as_deref(),
            Some("http://example.test/#not-a-comment")
        );
        assert_eq!(
            literal_string(f.blocks[0].attr("note")).as_deref(),
            Some("a // b")
        );
        assert_eq!(strip_comments("a # b\nc"), "a    \nc");
    }

    // "flags interpolated strings as dynamic"
    #[test]
    fn flags_interpolated_strings_as_dynamic() {
        let f = parse_hcl("locals {\n  a = \"x-${var.y}\"\n  b = \"plain\"\n}\n");
        assert!(matches!(
            f.blocks[0].attr("a"),
            Some(HclValue::String { dynamic: true, .. })
        ));
        assert_eq!(literal_string(f.blocks[0].attr("a")), None);
        assert_eq!(
            literal_string(f.blocks[0].attr("b")).as_deref(),
            Some("plain")
        );
    }

    // "reports an unclosed block instead of throwing"
    #[test]
    fn reports_an_unclosed_block() {
        let f = parse_hcl("resource \"a\" \"b\" {\n  x = 1\n");
        assert!(!f.errors.is_empty());
        assert!(f.errors[0].contains("never closed"));
    }

    // "records the source line of each block and walks nested blocks"
    #[test]
    fn records_the_line_and_walks_nested_blocks() {
        let f = parse_hcl(
            "\n\nresource \"a\" \"b\" {\n  provisioner \"local-exec\" {\n    command = \"x\"\n  }\n}\n",
        );
        assert_eq!(f.blocks[0].line, 3);
        let types: Vec<&str> = walk_blocks(&f.blocks)
            .iter()
            .map(|b| b.block_type.as_str())
            .collect();
        assert_eq!(types, vec!["resource", "provisioner"]);
    }
}
