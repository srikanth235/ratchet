//! The few `node:path` `posix` functions and JavaScript string helpers the TypeScript used, with their exact semantics.

/// `path.posix.normalize`.
pub fn normalize(path: &str) -> String {
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

/// `path.posix.join`.
pub fn join(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    if joined.is_empty() {
        ".".into()
    } else {
        normalize(&joined)
    }
}

/// `path.posix.dirname`.
pub fn dirname(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let has_root = path.starts_with('/');
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    let bytes = path.as_bytes();
    let mut i = bytes.len();
    while i > 1 {
        i -= 1;
        if bytes[i] == b'/' {
            if !matched_slash {
                end = Some(i);
                break;
            }
        } else {
            matched_slash = false;
        }
    }
    match end {
        None => {
            if has_root {
                "/".into()
            } else {
                ".".into()
            }
        }
        Some(1) if has_root => "//".into(),
        Some(e) => path[..e].to_owned(),
    }
}

/// `path.posix.basename`.
pub fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => trimmed[i + 1..].to_owned(),
        None => trimmed.to_owned(),
    }
}

/// `path.extname` for a POSIX path.
pub fn extname(path: &str) -> String {
    let base = basename(path);
    match base.rfind('.') {
        Some(0) | None => String::new(),
        Some(i) => {
            if base == ".." {
                String::new()
            } else {
                base[i..].to_owned()
            }
        }
    }
}

/// JavaScript's WhiteSpace and LineTerminator code points (what `trim()` strips); not Unicode's `White_Space`
/// (U+0085 is not one of them, U+FEFF is).
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\u{b}'
            | '\u{c}'
            | ' '
            | '\u{a0}'
            | '\u{feff}'
            | '\n'
            | '\r'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

pub fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_space)
}

pub fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_cases() {
        assert_eq!(normalize("a/./b//c"), "a/b/c");
        assert_eq!(normalize("./a//b/./c/"), "a/b/c/");
        assert_eq!(normalize("a/../../b"), "../b");
        assert_eq!(normalize(""), ".");
        assert_eq!(normalize("./"), "./");
    }

    #[test]
    fn dirname_basename() {
        assert_eq!(dirname(".devcontainer/Dockerfile"), ".devcontainer");
        assert_eq!(dirname("Dockerfile"), ".");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(dirname("a/b/c"), "a/b");
        assert_eq!(basename("a/b/c.json"), "c.json");
        assert_eq!(join(&["", "x"]), "x");
        assert_eq!(join(&[".devcontainer", ".."]), ".");
        assert_eq!(extname("a/trace.json?x"), ".json?x");
        assert_eq!(extname("a/trace"), "");
    }
}
