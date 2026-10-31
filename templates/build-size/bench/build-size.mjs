#!/usr/bin/env node
// Build time and bundle size evaluator (protected: bench/**). Runs your build and prints one JSON object on stdout:
//   { build_seconds, bundle_kb, bundle_gzip_kb, files, error_rate }
// `bundle_kb` is the total size of the files under `output_dir` that match `include` (raw bytes / 1024);
// `bundle_gzip_kb` is the same after gzip, per file. A failing build reports error_rate 1 and zero sizes.
//
// Configure it in bench/build-size.config.json:
//   clean        shell command run before the build (default removes nothing; e.g. "rm -rf dist")
//   build        the build command, e.g. "npm run build"
//   output_dir   directory the build writes to, relative to the repo root
//   include      file extensions that count as the shipped bundle (default js, mjs, cjs, css, html, wasm, json)
// Env overrides: BENCH_BUILD_COMMAND, BENCH_OUTPUT_DIR.
import { spawn } from 'node:child_process';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const here = dirname(fileURLToPath(import.meta.url));
const cfg = JSON.parse(readFileSync(join(here, 'build-size.config.json'), 'utf8'));
const build = process.env.BENCH_BUILD_COMMAND ?? cfg.build;
const outputDir = process.env.BENCH_OUTPUT_DIR ?? cfg.output_dir ?? 'dist';
const include = new Set((cfg.include ?? ['js', 'mjs', 'cjs', 'css', 'html', 'wasm', 'json']).map((e) => `.${e.replace(/^\./, '')}`));
if (!build) throw new Error('no build command (bench/build-size.config.json "build")');

const launcher = process.env.RATCHET_CANDIDATE_EXEC;
const quote = (s) => `'${s.replace(/'/g, `'\\''`)}'`;
const wrap = (command) => (launcher ? `${launcher} sh -c ${quote(command)}` : command);
const env = { ...process.env, CI: 'true' };
delete env.RATCHET_TRACE;

const sh = (command) =>
  new Promise((resolve) => {
    const child = spawn(wrap(command), { shell: true, stdio: ['ignore', 'ignore', 'inherit'], env });
    child.on('error', () => resolve(1));
    child.on('close', (code) => resolve(code ?? 1));
  });

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) yield* walk(p);
    else if (st.isFile()) yield p;
  }
}

if (cfg.clean) await sh(cfg.clean);
const t0 = process.hrtime.bigint();
const code = await sh(build);
const buildSeconds = Number(process.hrtime.bigint() - t0) / 1e9;

let raw = 0;
let gz = 0;
let files = 0;
if (code === 0) {
  try {
    for (const file of walk(outputDir)) {
      if (!include.has(extname(file))) continue;
      const bytes = readFileSync(file);
      raw += bytes.length;
      gz += gzipSync(bytes).length;
      files++;
    }
  } catch (err) {
    console.error(`cannot read ${outputDir}: ${err.message}`);
  }
}
const kb = (n) => Math.round((n / 1024) * 1000) / 1000;
console.log(
  JSON.stringify({
    build_seconds: Math.round(buildSeconds * 1000) / 1000,
    bundle_kb: kb(raw),
    bundle_gzip_kb: kb(gz),
    files,
    error_rate: code === 0 && files > 0 ? 0 : 1,
  }),
);
