//! Protected-path matching: a candidate that changes a protected path is blocked.
//!
//! Port of `packages/core/src/protected.ts`. The glob dialect and its quirks are the ones of the TypeScript
//! implementation, which compiled each glob to a JavaScript `RegExp`:
//!
//! - `*` any run of characters except `/`; `?` one character except `/`;
//! - `**` any run of characters including `/` (but not line terminators, like a regex `.`);
//!   `**/` also matches zero directories;
//! - `[abc]`, `[a-z]`, `[!abc]` (and `[^abc]`) character classes;
//! - `{a,b}` alternation (no nesting);
//! - everything else is literal; matching is case-sensitive; dotfiles are matched by wildcards
//!   (deliberately: protection should err on the side of blocking).
//!
//! Like the regex, matching works on UTF-16 code units, so `?` matches half of an emoji.
//!
//! Differences: where the TypeScript `new RegExp(...)` threw (an inverted range `[z-a]`;
//! an unterminated `{a` is literal in both), the Rust glob never matches ([`Glob::is_malformed`] tells), and `parse_config` rejects such a glob in
//! `protected:` so that it cannot silently protect nothing.

use serde::{Deserialize, Serialize};

/// A changed path that matched a protected glob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ProtectedViolation {
    pub path: String,
    pub pattern: String,
}

/// `path.posix.normalize`.
fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let is_absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if stack.last().is_some_and(|l| *l != "..") {
                    stack.pop();
                } else if !is_absolute {
                    stack.push("..");
                }
            }
            other => stack.push(other),
        }
    }
    let mut joined = stack.join("/");
    if joined.is_empty() {
        return if is_absolute {
            "/".into()
        } else if trailing {
            "./".into()
        } else {
            ".".into()
        };
    }
    if trailing {
        joined.push('/');
    }
    if is_absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// Strip `^(\.\/)+` from the front.
fn strip_dot_slash(mut s: &str) -> &str {
    while let Some(rest) = s.strip_prefix("./") {
        s = rest;
    }
    s
}

/// Normalise to a repo-relative POSIX path: `\` -> `/`, strip leading `./` and `/`, collapse `a/../b`.
pub fn normalize_repo_path(p: &str) -> String {
    let s = posix_normalize(&p.replace('\\', "/"));
    let s = strip_dot_slash(&s).trim_start_matches('/');
    if s == "." {
        String::new()
    } else {
        s.to_owned()
    }
}

// ---------------------------------------------------------------------------------------------------------
// glob compilation: parse into a small AST, compile to a Thompson-NFA program, run it without backtracking

#[derive(Debug, Clone)]
enum Node {
    Lit(u16),
    /// `[^/]*`
    Star,
    /// `.*`
    DoubleStar,
    /// `(?:.*/)?`
    DoubleStarSlash,
    /// `[^/]`
    NotSlash,
    Class {
        negated: bool,
        ranges: Vec<(u16, u16)>,
    },
    /// `[^]`: any code unit
    Any,
    Group(Vec<Vec<Node>>),
    /// Never matches (a valid pattern that matches nothing, e.g. `[]`)
    Fail,
    /// Never matches, and the TypeScript `new RegExp(...)` threw on it: the glob is malformed
    Invalid,
}

#[derive(Debug, Clone)]
enum Inst {
    Lit(u16),
    NotSlash,
    /// `.`: anything but a line terminator
    Dot,
    Any,
    Class {
        negated: bool,
        ranges: Vec<(u16, u16)>,
    },
    Split(usize, usize),
    Jmp(usize),
    Fail,
    Match,
}

fn is_line_terminator(c: u16) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

fn unit(c: char) -> u16 {
    c as u32 as u16
}

/// Parse the body of a `[...]` class the way the equivalent JavaScript regex class reads it (backslashes were
/// doubled by the TypeScript, so they are literal here).
fn parse_class(body: &[u16]) -> Node {
    let mut body: Vec<u16> = body.to_vec();
    if body.first() == Some(&unit('!')) {
        body[0] = unit('^');
    }
    let negated = body.first() == Some(&unit('^'));
    let items = if negated { &body[1..] } else { &body[..] };
    // `[]...]` closes immediately in JavaScript: an empty class, which can never match
    if items.first() == Some(&unit(']')) {
        return if negated {
            // `[^]` matches any code unit, the rest is read as plain text followed by a literal `]`
            let mut seq = vec![Node::Any];
            seq.extend(items.iter().map(|c| Node::Lit(*c)));
            seq.push(Node::Lit(unit(']')));
            Node::Group(vec![seq])
        } else {
            Node::Fail
        };
    }
    let mut ranges: Vec<(u16, u16)> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let lo = items[i];
        if i + 2 < items.len() && items[i + 1] == unit('-') {
            let hi = items[i + 2];
            if lo > hi {
                return Node::Invalid; // JavaScript: "Range out of order in character class"
            }
            ranges.push((lo, hi));
            i += 3;
        } else {
            ranges.push((lo, lo));
            i += 1;
        }
    }
    if items.is_empty() {
        // `[^]` (from `[!]`) matches anything; `[]` cannot occur here
        return if negated { Node::Any } else { Node::Fail };
    }
    Node::Class { negated, ranges }
}

fn parse_glob(glob: &[u16]) -> Vec<Node> {
    let g = |i: usize| glob.get(i).copied();
    let (star, qmark, lbracket, rbracket, lbrace, rbrace, comma, slash) = (
        unit('*'),
        unit('?'),
        unit('['),
        unit(']'),
        unit('{'),
        unit('}'),
        unit(','),
        unit('/'),
    );
    // stack of open groups: finished alternatives + the one being built
    let mut stack: Vec<(Vec<Vec<Node>>, Vec<Node>)> = vec![(Vec::new(), Vec::new())];
    let mut in_brace = false;
    let mut i = 0;
    while i < glob.len() {
        let c = glob[i];
        let node = if c == star {
            if g(i + 1) == Some(star) {
                i += 1;
                if g(i + 1) == Some(slash) {
                    i += 1;
                    Node::DoubleStarSlash
                } else {
                    Node::DoubleStar
                }
            } else {
                Node::Star
            }
        } else if c == qmark {
            Node::NotSlash
        } else if c == lbracket {
            // the closing `]` is searched from i + 2, so `[]` and `[!]` are read as classes
            let close = (i + 2 <= glob.len())
                .then(|| glob[i + 2..].iter().position(|u| *u == rbracket))
                .flatten();
            match close {
                None => Node::Lit(c),
                Some(rel) => {
                    let close = i + 2 + rel;
                    let node = parse_class(&glob[i + 1..close]);
                    i = close;
                    node
                }
            }
        } else if c == lbrace && !in_brace && glob[i..].contains(&rbrace) {
            in_brace = true;
            stack.push((Vec::new(), Vec::new()));
            i += 1;
            continue;
        } else if c == rbrace && in_brace {
            in_brace = false;
            if let Some((mut alts, last)) = stack.pop() {
                alts.push(last);
                if let Some(top) = stack.last_mut() {
                    top.1.push(Node::Group(alts));
                }
            }
            i += 1;
            continue;
        } else if c == comma && in_brace {
            if let Some(top) = stack.last_mut() {
                let done = std::mem::take(&mut top.1);
                top.0.push(done);
            }
            i += 1;
            continue;
        } else {
            Node::Lit(c)
        };
        if let Some(top) = stack.last_mut() {
            top.1.push(node);
        }
        i += 1;
    }
    if stack.len() > 1 {
        // an unterminated `{`: the TypeScript regex failed to compile
        return vec![Node::Invalid];
    }
    stack.pop().map(|(_, cur)| cur).unwrap_or_default()
}

fn has_invalid(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| match n {
        Node::Invalid => true,
        Node::Group(alts) => alts.iter().any(|a| has_invalid(a)),
        _ => false,
    })
}

fn emit_seq(nodes: &[Node], prog: &mut Vec<Inst>) {
    for n in nodes {
        emit_node(n, prog);
    }
}

fn emit_star(inner: Inst, prog: &mut Vec<Inst>) {
    // L1: split(L2, L3); L2: inner; jmp L1; L3:
    let l1 = prog.len();
    prog.push(Inst::Split(l1 + 1, 0));
    prog.push(inner);
    prog.push(Inst::Jmp(l1));
    let end = prog.len();
    prog[l1] = Inst::Split(l1 + 1, end);
}

fn emit_node(n: &Node, prog: &mut Vec<Inst>) {
    match n {
        Node::Lit(c) => prog.push(Inst::Lit(*c)),
        Node::NotSlash => prog.push(Inst::NotSlash),
        Node::Any => prog.push(Inst::Any),
        Node::Fail | Node::Invalid => prog.push(Inst::Fail),
        Node::Class { negated, ranges } => prog.push(Inst::Class {
            negated: *negated,
            ranges: ranges.clone(),
        }),
        Node::Star => emit_star(Inst::NotSlash, prog),
        Node::DoubleStar => emit_star(Inst::Dot, prog),
        Node::DoubleStarSlash => {
            // (?:.*/)?
            let split = prog.len();
            prog.push(Inst::Split(split + 1, 0));
            emit_star(Inst::Dot, prog);
            prog.push(Inst::Lit(unit('/')));
            let end = prog.len();
            prog[split] = Inst::Split(split + 1, end);
        }
        Node::Group(alts) => {
            let mut jumps = Vec::new();
            for (i, alt) in alts.iter().enumerate() {
                if i + 1 < alts.len() {
                    let split = prog.len();
                    prog.push(Inst::Split(split + 1, 0));
                    emit_seq(alt, prog);
                    jumps.push(prog.len());
                    prog.push(Inst::Jmp(0));
                    let next = prog.len();
                    prog[split] = Inst::Split(split + 1, next);
                } else {
                    emit_seq(alt, prog);
                }
            }
            let end = prog.len();
            for j in jumps {
                prog[j] = Inst::Jmp(end);
            }
        }
    }
}

/// A compiled glob (the equivalent of `globToRegExp`): matches a whole string, anchored at both ends.
#[derive(Debug, Clone)]
pub struct Glob {
    prog: Vec<Inst>,
    malformed: bool,
}

impl Glob {
    /// Compile a glob. Never fails: see the module docs for what an invalid glob does.
    pub fn new(glob: &str) -> Self {
        let units: Vec<u16> = glob.encode_utf16().collect();
        let nodes = parse_glob(&units);
        let mut prog = Vec::new();
        emit_seq(&nodes, &mut prog);
        prog.push(Inst::Match);
        Glob {
            prog,
            malformed: has_invalid(&nodes),
        }
    }

    /// Is the glob malformed (an inverted range `[z-a]`, an unterminated `{a`)? Such a glob never matches, so a
    /// protected list holding one protects nothing: the configuration parser rejects it instead.
    pub fn is_malformed(&self) -> bool {
        self.malformed
    }

    fn add(&self, set: &mut Vec<usize>, seen: &mut [bool], pc: usize) {
        if seen[pc] {
            return;
        }
        seen[pc] = true;
        match &self.prog[pc] {
            Inst::Split(a, b) => {
                self.add(set, seen, *a);
                self.add(set, seen, *b);
            }
            Inst::Jmp(t) => self.add(set, seen, *t),
            Inst::Fail => {}
            _ => set.push(pc),
        }
    }

    /// Does the whole of `text` match the glob?
    pub fn is_match(&self, text: &str) -> bool {
        let n = self.prog.len();
        let mut current: Vec<usize> = Vec::new();
        let mut seen = vec![false; n];
        self.add(&mut current, &mut seen, 0);
        for c in text.encode_utf16() {
            let mut next: Vec<usize> = Vec::new();
            let mut seen = vec![false; n];
            for &pc in &current {
                let ok = match &self.prog[pc] {
                    Inst::Lit(x) => *x == c,
                    Inst::NotSlash => c != unit('/'),
                    Inst::Dot => !is_line_terminator(c),
                    Inst::Any => true,
                    Inst::Class { negated, ranges } => {
                        ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c)) != *negated
                    }
                    Inst::Match | Inst::Split(..) | Inst::Jmp(_) | Inst::Fail => false,
                };
                if ok {
                    self.add(&mut next, &mut seen, pc + 1);
                }
            }
            if next.is_empty() {
                return false;
            }
            current = next;
        }
        current
            .iter()
            .any(|pc| matches!(self.prog[*pc], Inst::Match))
    }
}

/// Compile a glob to an anchored matcher (`globToRegExp`).
pub fn glob_to_regexp(glob: &str) -> Glob {
    Glob::new(glob)
}

/// The glob as `matchesGlob` reads it: `\` -> `/`, leading `./` and `/` and trailing `/` stripped.
fn clean_glob(glob: &str) -> String {
    let g = glob.replace('\\', "/");
    let g = strip_dot_slash(&g).trim_start_matches('/');
    g.trim_end_matches('/').to_owned()
}

fn matches_compiled(path: &str, g: &str, re: &Glob) -> bool {
    let p = normalize_repo_path(path);
    if p.is_empty() || g.is_empty() {
        return false;
    }
    let parts: Vec<&str> = p.split('/').collect();
    (1..=parts.len()).any(|n| re.is_match(&parts[..n].join("/")))
}

/// Is `glob` malformed, as `matches_glob` reads it? See [`Glob::is_malformed`].
pub fn is_malformed_glob(glob: &str) -> bool {
    Glob::new(&clean_glob(glob)).is_malformed()
}

/// Does `path` match `glob`? A path also matches when the glob matches one of its parent directories (so
/// `bench` or `bench/` protects `bench/k6/search.js`). Patterns match against the full repo-relative path:
/// `ratchet.yaml` only matches the root file; use `**/ratchet.yaml` for any depth.
pub fn matches_glob(path: &str, glob: &str) -> bool {
    let g = clean_glob(glob);
    if g.is_empty() {
        return false;
    }
    matches_compiled(path, &g, &Glob::new(&g))
}

/// Return one violation per changed path that matches any protected glob (the first matching pattern is
/// reported). Pass both old and new paths for renames.
pub fn find_protected_violations<P: AsRef<str>, G: AsRef<str>>(
    changed_paths: &[P],
    protected_globs: &[G],
) -> Vec<ProtectedViolation> {
    let compiled: Vec<(String, Glob)> = protected_globs
        .iter()
        .map(|g| {
            let cleaned = clean_glob(g.as_ref());
            let re = Glob::new(&cleaned);
            (cleaned, re)
        })
        .collect();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in changed_paths {
        let norm = normalize_repo_path(path.as_ref());
        if !seen.insert(norm.clone()) {
            continue;
        }
        let hit = protected_globs
            .iter()
            .zip(&compiled)
            .find(|(_, (cleaned, re))| matches_compiled(&norm, cleaned, re))
            .map(|(original, _)| original.as_ref().to_owned());
        if let Some(pattern) = hit {
            out.push(ProtectedViolation {
                path: norm,
                pattern,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // protected.test.ts: normalizeRepoPath "normalises"
    #[test]
    fn normalizes() {
        assert_eq!(normalize_repo_path("./bench/a.js"), "bench/a.js");
        assert_eq!(normalize_repo_path("/bench/a.js"), "bench/a.js");
        assert_eq!(normalize_repo_path("bench\\a.js"), "bench/a.js");
        assert_eq!(normalize_repo_path("src/../bench/a.js"), "bench/a.js");
        assert_eq!(normalize_repo_path("bench//a.js"), "bench/a.js");
    }

    #[test]
    fn normalize_edge_cases_follow_node() {
        assert_eq!(normalize_repo_path(""), "");
        assert_eq!(normalize_repo_path("."), "");
        assert_eq!(normalize_repo_path("./"), "");
        assert_eq!(normalize_repo_path("a/.."), "");
        assert_eq!(normalize_repo_path("../a"), "../a");
        assert_eq!(normalize_repo_path("/../a"), "a");
        assert_eq!(normalize_repo_path("a/b/../../.."), "..");
        assert_eq!(normalize_repo_path("bench/"), "bench/");
        assert_eq!(normalize_repo_path("././a"), "a");
    }

    // "literal paths match exactly (root only)"
    #[test]
    fn literal_paths_match_exactly() {
        assert!(matches_glob("ratchet.yaml", "ratchet.yaml"));
        assert!(!matches_glob("sub/ratchet.yaml", "ratchet.yaml"));
        assert!(!matches_glob("ratchet.yaml.bak", "ratchet.yaml"));
        assert!(!matches_glob("xratchet.yaml", "ratchet.yaml"));
    }

    // "dir/** matches everything underneath at any depth"
    #[test]
    fn dir_double_star_matches_everything_underneath() {
        assert!(matches_glob("bench/k6/search.js", "bench/**"));
        assert!(matches_glob("bench/a", "bench/**"));
        assert!(!matches_glob("benchmark/a", "bench/**"));
        assert!(!matches_glob("src/bench/a", "bench/**"));
        assert!(matches_glob(
            "tests/contract/deep/er/x.py",
            "tests/contract/**"
        ));
        assert!(!matches_glob("tests/unit/x.py", "tests/contract/**"));
    }

    // "single * does not cross directories"
    #[test]
    fn single_star_does_not_cross_directories() {
        assert!(matches_glob("src/a.ts", "src/*.ts"));
        assert!(!matches_glob("src/deep/a.ts", "src/*.ts"));
        assert!(!matches_glob("src/a.js", "src/*.ts"));
        assert!(matches_glob("src/.hidden.ts", "src/*.ts"));
    }

    // "? matches exactly one non-slash char"
    #[test]
    fn question_mark_matches_one_non_slash_char() {
        assert!(matches_glob("a1.txt", "a?.txt"));
        assert!(!matches_glob("a.txt", "a?.txt"));
        assert!(!matches_glob("a/.txt", "a?.txt"));
    }

    // "**/ matches zero or more directories"
    #[test]
    fn double_star_slash_matches_zero_or_more_directories() {
        assert!(matches_glob("ratchet.yaml", "**/ratchet.yaml"));
        assert!(matches_glob("a/b/ratchet.yaml", "**/ratchet.yaml"));
        assert!(!matches_glob("a/ratchet.yml", "**/ratchet.yaml"));
        assert!(matches_glob("src/x/y/test_a.py", "src/**/test_*.py"));
        assert!(matches_glob("src/test_a.py", "src/**/test_*.py"));
    }

    // "character classes and braces"
    #[test]
    fn character_classes_and_braces() {
        assert!(matches_glob("a1", "a[0-9]"));
        assert!(!matches_glob("ab", "a[0-9]"));
        assert!(matches_glob("ab", "a[!0-9]"));
        assert!(matches_glob("src/a.ts", "src/*.{ts,js}"));
        assert!(matches_glob("src/a.js", "src/*.{ts,js}"));
        assert!(!matches_glob("src/a.py", "src/*.{ts,js}"));
    }

    // "treats regex metacharacters literally"
    #[test]
    fn treats_regex_metacharacters_literally() {
        assert!(matches_glob("a+b.txt", "a+b.txt"));
        assert!(!matches_glob("aab.txt", "a+b.txt"));
        assert!(matches_glob("a(b).txt", "a(b).txt"));
        assert!(matches_glob("a$.txt", "a$.txt"));
        assert!(!matches_glob("axtxt", "a.txt"));
        assert!(matches_glob("x[", "x["));
    }

    // "a pattern matching a parent directory protects its children"
    #[test]
    fn a_pattern_matching_a_parent_directory_protects_its_children() {
        assert!(matches_glob("bench/k6/s.js", "bench"));
        assert!(matches_glob("bench/k6/s.js", "bench/"));
        assert!(matches_glob("bench/k6/s.js", "./bench/k6"));
        assert!(!matches_glob("benchx/s.js", "bench"));
    }

    // "normalises the candidate path before matching (no ../ escape)"
    #[test]
    fn normalises_the_candidate_path_before_matching() {
        assert!(matches_glob("./src/../bench/a.js", "bench/**"));
        assert!(matches_glob("/bench/a.js", "bench/**"));
        assert!(matches_glob("bench\\a.js", "bench/**"));
    }

    // "is case-sensitive and ignores empty inputs"
    #[test]
    fn is_case_sensitive_and_ignores_empty_inputs() {
        assert!(!matches_glob("Bench/a", "bench/**"));
        assert!(!matches_glob("", "bench/**"));
        assert!(!matches_glob("a", ""));
    }

    // "globToRegExp is anchored"
    #[test]
    fn glob_to_regexp_is_anchored() {
        assert!(!glob_to_regexp("a*").is_match("xa"));
        assert!(glob_to_regexp("a*").is_match("ab"));
    }

    #[test]
    fn javascript_regex_quirks_are_kept() {
        // `[!]` is `[^]`: any character, even a slash
        assert!(glob_to_regexp("a[!]b").is_match("a/b"));
        // `.*` does not cross line terminators, `[^/]*` does
        assert!(glob_to_regexp("a**b").is_match("axyzb"));
        assert!(!glob_to_regexp("a**b").is_match("a\nb"));
        assert!(glob_to_regexp("a*b").is_match("a\nb"));
        // UTF-16 code units: `?` is half an astral character
        assert!(!glob_to_regexp("a?b").is_match("a\u{1F600}b"));
        assert!(glob_to_regexp("a??b").is_match("a\u{1F600}b"));
        // braces: no nesting, empty alternatives allowed
        assert!(glob_to_regexp("x{a,}y").is_match("xy"));
        assert!(glob_to_regexp("x{a,b}y").is_match("xby"));
    }

    #[test]
    fn invalid_globs_never_match() {
        assert!(!glob_to_regexp("[z-a]").is_match("m"));
        assert!(!glob_to_regexp("{a,b").is_match("a"));
        assert!(!glob_to_regexp("[]a]").is_match("a"));
    }

    fn globs() -> Vec<&'static str> {
        vec!["bench/**", "ratchet.yaml", "tests/contract/**"]
    }

    // "returns nothing when no path is protected"
    #[test]
    fn returns_nothing_when_no_path_is_protected() {
        assert_eq!(
            find_protected_violations(&["src/app.ts", "README.md"], &globs()),
            vec![]
        );
        assert_eq!(
            find_protected_violations::<&str, &str>(&[], &globs()),
            vec![]
        );
        assert_eq!(
            find_protected_violations(&["bench/x"], &[] as &[&str]),
            vec![]
        );
    }

    // "reports each violating path with the first matching pattern"
    #[test]
    fn reports_each_violating_path_with_the_first_matching_pattern() {
        let v = |path: &str, pattern: &str| ProtectedViolation {
            path: path.into(),
            pattern: pattern.into(),
        };
        assert_eq!(
            find_protected_violations(
                &[
                    "src/app.ts",
                    "bench/k6/search.js",
                    "ratchet.yaml",
                    "tests/contract/a.py"
                ],
                &globs()
            ),
            vec![
                v("bench/k6/search.js", "bench/**"),
                v("ratchet.yaml", "ratchet.yaml"),
                v("tests/contract/a.py", "tests/contract/**"),
            ]
        );
    }

    // "deduplicates paths after normalisation"
    #[test]
    fn deduplicates_paths_after_normalisation() {
        assert_eq!(
            find_protected_violations(&["ratchet.yaml", "./ratchet.yaml"], &globs()).len(),
            1
        );
    }
}
