import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, describe, expect, it } from "vitest";

// The bench image installs the PREBUILT Rust launcher (crates/ratchet-bench, bin ratchet-candidate-launcher; CI artifact
// ratchet-candidate-launcher-<arch>) with infra/images/scripts/35-candidate-launcher.sh: sha256 check, native static ELF check,
// then 4755 root in a 0700 root directory. This runs the real script in a scratch prefix (RATCHET_PREFIX) against synthetic ELF
// files, so it needs no compiler and no cargo build. The launcher's own behavior (uids, no_new_privs, environment, exit codes)
// is tested by crates/ratchet-bench/tests/launcher.rs; the install itself needs root (`install -o root`), so those cases skip
// elsewhere, like the old behavior tests did. CI (images.yml) also installs the real binary the same way.

const here = dirname(fileURLToPath(import.meta.url));
const scripts = join(here, "..", "..", "images", "scripts");
const script = join(scripts, "35-candidate-launcher.sh");
const root = typeof process.getuid === "function" && process.getuid() === 0;
const machine = process.arch === "arm64" ? 183 : process.arch === "x64" ? 62 : 0;
const otherMachine = machine === 62 ? 183 : 62;

const dir = mkdtempSync(join(tmpdir(), "ratchet-launcher-install-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));
let n = 0;

/** A minimal 64-bit little-endian ELF: header, `types` program headers, then the configuration path the real binary embeds. */
function elf(opts: { machine?: number; interp?: boolean; type?: number; conf?: string } = {}): Buffer {
  const phnum = opts.interp ? 3 : 2;
  const b = Buffer.alloc(64 + phnum * 56);
  Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1]).copy(b, 0);
  b.writeUInt16LE(opts.type ?? 3, 16); // ET_DYN: a static-pie executable
  b.writeUInt16LE(opts.machine ?? machine, 18);
  b.writeBigUInt64LE(64n, 32);
  b.writeUInt16LE(56, 54);
  b.writeUInt16LE(phnum, 56);
  const types = opts.interp ? [1, 3, 2] : [1, 2]; // PT_LOAD, [PT_INTERP], PT_DYNAMIC
  types.forEach((t, i) => b.writeUInt32LE(t, 64 + i * 56));
  return Buffer.concat([b, Buffer.from(opts.conf ?? "/ratchet/launcher.conf\0")]);
}

const sha = (b: Buffer) => createHash("sha256").update(b).digest("hex");

function install(content: Buffer, env: Record<string, string | undefined> = {}) {
  const base = join(dir, `t${n++}`);
  mkdirSync(base);
  const bin = join(base, "launcher");
  writeFileSync(bin, content);
  const prefix = join(base, "prefix");
  const r = spawnSync("bash", [script], {
    encoding: "utf8",
    env: { PATH: process.env.PATH, RATCHET_PREFIX: prefix, RATCHET_LAUNCHER_BINARY: bin, RATCHET_LAUNCHER_SHA256: sha(content), ...env },
  });
  return { r, dest: join(prefix, "libexec", "ratchet"), bin };
}

describe("35-candidate-launcher.sh installs the prebuilt launcher", () => {
  it.skipIf(!root || machine === 0)("installs it as ratchet-candidate-ctr: mode 4755 root:root in a 0700 root:root directory", () => {
    const { r, dest } = install(elf());
    expect(r.stderr).toContain("verify the launcher");
    expect(r.status, r.stderr).toBe(0);
    const f = statSync(join(dest, "ratchet-candidate-ctr"));
    expect(f.mode & 0o7777).toBe(0o4755);
    expect([f.uid, f.gid]).toEqual([0, 0]);
    const d = statSync(dest);
    expect(d.mode & 0o7777).toBe(0o700);
    expect([d.uid, d.gid]).toEqual([0, 0]);
  });

  it.skipIf(machine === 0)("fails on a SHA-256 mismatch and installs nothing", () => {
    const { r, dest } = install(elf(), { RATCHET_LAUNCHER_SHA256: "0".repeat(64) });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("sha256 mismatch");
    expect(() => statSync(join(dest, "ratchet-candidate-ctr"))).toThrow();
  });

  it.skipIf(machine === 0)("is not fooled by a digest of different bytes (checks the file it installs)", () => {
    const { r } = install(elf(), { RATCHET_LAUNCHER_SHA256: sha(elf({ conf: "/ratchet/launcher.conf\0x" })) });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("sha256 mismatch");
  });

  it.each([["unset", undefined], ["empty", ""], ["short", "abc"], ["not hex", "z".repeat(64)]])("requires a well-formed digest (%s)", (_n, v) => {
    const { r } = install(elf(), { RATCHET_LAUNCHER_SHA256: v });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("RATCHET_LAUNCHER_SHA256");
  });

  it("fails when the binary is missing", () => {
    const { r } = install(elf(), { RATCHET_LAUNCHER_BINARY: join(dir, "nope") });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("is missing");
  });

  it.skipIf(machine === 0)("refuses a binary built for the other architecture", () => {
    const c = elf({ machine: otherMachine });
    const { r } = install(c);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("not a 64-bit ELF executable for this machine");
  });

  it("refuses something that is not an ELF executable", () => {
    const c = Buffer.from("#!/bin/sh\necho /ratchet/launcher.conf\n");
    const { r } = install(c);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("not a 64-bit ELF executable");
  });

  it.skipIf(machine === 0)("refuses a dynamically linked binary (it has a program interpreter)", () => {
    const { r } = install(elf({ interp: true }));
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("dynamically linked");
  });

  it.skipIf(machine === 0)("refuses a binary that does not name /ratchet/launcher.conf", () => {
    const { r } = install(elf({ conf: "/etc/other.conf\0" }));
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("/ratchet/launcher.conf");
  });
});
