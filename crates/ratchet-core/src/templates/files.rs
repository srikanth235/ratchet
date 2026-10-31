//! The template files, embedded at compile time.
//!
//! The files live in `templates/<id>/**` at the repository root and are embedded with `include_dir!`
//! (path relative to this crate). There is no generated file to keep in sync.

use include_dir::{Dir, DirEntry, include_dir};

static TEMPLATES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../templates");

/// Template ids that have files, sorted.
pub fn template_ids_with_files() -> Vec<String> {
    let mut ids: Vec<String> = TEMPLATES
        .dirs()
        .filter_map(|d| {
            d.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_owned)
        })
        .collect();
    ids.sort();
    ids
}

/// The files of template `id` as `(repo-relative POSIX path, content)`, in the order the original TypeScript
/// generator produced them: entries sorted by name at each directory level, directories expanded in place.
/// `RATCHET.md` still has its `{{placeholders}}`. Empty for an unknown id.
pub fn template_files(id: &str) -> Vec<(String, &'static str)> {
    let Some(dir) = TEMPLATES.get_dir(id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk(dir, dir.path().to_str().unwrap_or(""), &mut out);
    out
}

fn name_of(e: &DirEntry<'static>) -> String {
    e.path()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_owned()
}

fn walk(dir: &'static Dir<'static>, root: &str, out: &mut Vec<(String, &'static str)>) {
    let mut entries: Vec<&'static DirEntry<'static>> = dir.entries().iter().collect();
    entries.sort_by_key(|e| name_of(e));
    for e in entries {
        match e {
            DirEntry::Dir(d) => walk(d, root, out),
            DirEntry::File(f) => {
                let full = f.path().to_str().unwrap_or("").replace('\\', "/");
                let rel = full
                    .strip_prefix(root)
                    .unwrap_or(&full)
                    .trim_start_matches('/')
                    .to_owned();
                if let Some(text) = f.contents_utf8() {
                    out.push((rel, text));
                }
            }
        }
    }
}
