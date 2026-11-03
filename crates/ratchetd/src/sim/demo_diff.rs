//! A believable winning change for the simulation.
//!
//! The simulation has no git behind it, so a demo used to answer "no git access" and the Result screen had nothing to
//! show. These are small, fixed diffs (a different one per kind of demo) that look like what the agents of the demo
//! would send: the file list and the first hunk of the most-changed file are rendered exactly like a real diff. The line
//! counts are counted from the patch, so the totals can never disagree with what is shown.

use ratchet_core::ports::FileDiff;

/// One file of a demo diff. The hunk header is written as `@@ -old +new @@ section` (start lines only); the line counts of
/// the header and the file's totals are counted from the body, so they can never disagree with what is shown.
fn file(path: &str, patch: &str) -> FileDiff {
    let (head, body) = patch.split_once('\n').unwrap_or((patch, ""));
    let (mut additions, mut deletions, mut context) = (0u64, 0u64, 0u64);
    for line in body.lines() {
        match line.as_bytes().first() {
            Some(b'+') => additions += 1,
            Some(b'-') => deletions += 1,
            _ => context += 1,
        }
    }
    let (ranges, section) = head
        .strip_prefix("@@")
        .and_then(|rest| rest.split_once("@@"))
        .unwrap_or((" -1 +1 ", ""));
    let mut starts = ranges.split_whitespace();
    let old = starts.next().unwrap_or("-1").trim_start_matches('-');
    let new = starts.next().unwrap_or("+1").trim_start_matches('+');
    FileDiff {
        path: path.to_owned(),
        additions,
        deletions,
        binary: false,
        patch: format!(
            "@@ -{old},{} +{new},{} @@{section}\n{body}",
            deletions + context,
            additions + context
        ),
    }
}

/// The diff of the best verified commit against the base ref, for the demo that `repo` and `metric` identify.
pub(super) fn files_for(repo: &str, metric: &str) -> Vec<FileDiff> {
    match repo {
        "acme/search-api" => search(),
        "acme/ingest" => ingest(),
        "acme/edge-worker" => edge(),
        "acme/web" => web(),
        _ if metric.contains("duration") || metric.contains("latency") => search(),
        _ => generic(),
    }
}

fn search() -> Vec<FileDiff> {
    vec![
        file(
            "src/search/rank.ts",
            "@@ -41 +41 @@ export function search(index: Index, query: ParsedQuery, page: Page): Hit[] {\n \
             const terms = query.terms;\n \
             const scored: Scored[] = [];\n-  for (const doc of index.docs) {\n-    const score = scoreDoc(doc, terms);\n-    if (score > 0) scored.push({ doc, score });\n-  }\n-  scored.sort((a, b) => b.score - a.score);\n-  return scored.slice(page.offset, page.offset + page.size).map(toHit);\n+  const weights = new Float64Array(terms.length);\n+  for (let i = 0; i < terms.length; i++) weights[i] = terms[i].weight;\n+  for (const doc of index.docs) {\n+    const score = scoreDoc(doc, terms, weights);\n+    if (score > 0) scored.push({ doc, score });\n+  }\n+  const top = selectTop(scored, page.offset + page.size);\n+  return top.slice(page.offset).map(toHit);\n }\n",
        ),
        file(
            "src/search/select.ts",
            "@@ -0 +1 @@\n+/** The k best by score, without sorting everything (a bounded heap). */\n+export function selectTop(items: Scored[], k: number): Scored[] {\n+  const heap: Scored[] = [];\n+  for (const item of items) {\n+    if (heap.length < k) {\n+      heap.push(item);\n+      siftUp(heap, heap.length - 1);\n+    } else if (item.score > heap[0].score) {\n+      heap[0] = item;\n+      siftDown(heap, 0);\n+    }\n+  }\n+  return heap.sort((a, b) => b.score - a.score);\n+}\n",
        ),
        file(
            "src/search/parse.ts",
            "@@ -8 +8 @@ const cache = new Map<string, Filter>();\n \n export function parseFilter(raw: string): Filter {\n-  return compile(raw);\n+  const hit = cache.get(raw);\n+  if (hit) return hit;\n+  const filter = compile(raw);\n+  cache.set(raw, filter);\n+  return filter;\n }\n",
        ),
    ]
}

fn ingest() -> Vec<FileDiff> {
    vec![
        file(
            "src/ingest/writer.ts",
            "@@ -22 +22 @@ export class SegmentWriter {\n   private buffer: Buffer[] = [];\n+  private size = 0;\n \n   write(record: Buffer): void {\n-    this.sink.write(record);\n+    this.buffer.push(record);\n+    this.size += record.length;\n+    if (this.size >= SEGMENT_BYTES) this.flush();\n   }\n+\n+  flush(): void {\n+    if (this.size === 0) return;\n+    this.sink.write(Buffer.concat(this.buffer, this.size));\n+    this.buffer = [];\n+    this.size = 0;\n+  }\n }\n",
        ),
        file(
            "src/ingest/config.ts",
            "@@ -3 +3 @@\n export const BATCH = 500;\n-export const SOCKET_BUFFER = 16 * 1024;\n+export const SOCKET_BUFFER = 256 * 1024;\n+export const SEGMENT_BYTES = 4096;\n",
        ),
    ]
}

fn edge() -> Vec<FileDiff> {
    vec![
        file(
            "src/worker/templates.ts",
            "@@ -5 +5 @@ import { compile } from './engine';\n-import { engine } from './engine';\n-\n-const templates = loadAll(engine);\n+let templates: Templates | undefined;\n \n-export function render(name: string, data: Data): string {\n-  return templates[name](data);\n+export async function render(name: string, data: Data): Promise<string> {\n+  templates ??= await loadAll(await import('./engine'));\n+  return templates[name](data);\n }\n",
        ),
        file(
            "src/worker/index.ts",
            "@@ -1 +1 @@\n-import './templates';\n import { route } from './router';\n \n export default { fetch: route };\n",
        ),
    ]
}

fn web() -> Vec<FileDiff> {
    vec![file(
        "src/render/page.ts",
        "@@ -14 +14 @@ export function renderPage(model: PageModel): string {\n-  const parts = model.blocks.map((b) => renderBlock(b));\n-  return layout(parts.join(''));\n+  let html = '';\n+  for (const block of model.blocks) html += renderBlock(block);\n+  return layout(html);\n }\n",
    )]
}

fn generic() -> Vec<FileDiff> {
    vec![file(
        "src/hot_path.ts",
        "@@ -10 +10 @@ export function handle(input: Input): Output {\n-  const rows = load(input).map(normalize);\n-  return summarize(rows);\n+  const key = cacheKey(input);\n+  const cached = cache.get(key);\n+  if (cached) return cached;\n+  const out = summarize(load(input).map(normalize));\n+  cache.set(key, out);\n+  return out;\n }\n",
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_demo_has_a_diff_whose_counts_match_its_patch() {
        for repo in [
            "acme/search-api",
            "acme/ingest",
            "acme/edge-worker",
            "acme/web",
            "somebody/else",
        ] {
            let files = files_for(repo, "x");
            assert!(!files.is_empty(), "{repo}");
            for f in &files {
                assert!(!f.binary && f.patch.starts_with("@@"), "{repo} {}", f.path);
                let adds = f.patch.lines().filter(|l| l.starts_with('+')).count() as u64;
                let dels = f.patch.lines().filter(|l| l.starts_with('-')).count() as u64;
                assert_eq!((f.additions, f.deletions), (adds, dels), "{}", f.path);
                assert!(f.additions + f.deletions > 0, "{}", f.path);
            }
        }
    }

    #[test]
    fn the_hunk_header_counts_agree_with_the_hunk() {
        // `@@ -a,b +c,d @@`: b lines of context and deletions, d lines of context and additions.
        for repo in [
            "acme/search-api",
            "acme/ingest",
            "acme/edge-worker",
            "acme/web",
            "x/y",
        ] {
            for f in files_for(repo, "latency") {
                let mut lines = f.patch.lines();
                let header = lines.next().unwrap();
                let nums: Vec<u64> = header
                    .split_whitespace()
                    .skip(1)
                    .take(2)
                    .map(|part| {
                        let part = part.trim_start_matches(['-', '+']);
                        part.split(',').nth(1).map_or(1, |n| n.parse().unwrap())
                    })
                    .collect();
                let (mut old, mut new) = (0, 0);
                for line in lines {
                    match line.as_bytes().first() {
                        Some(b'+') => new += 1,
                        Some(b'-') => old += 1,
                        _ => {
                            old += 1;
                            new += 1;
                        }
                    }
                }
                assert_eq!((old, new), (nums[0], nums[1]), "{} {}", repo, f.path);
            }
        }
    }
}
