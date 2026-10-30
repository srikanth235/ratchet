import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, describe, expect, it } from "vitest";

// The setuid-root launcher the harness uses inside the bench container (infra/images/scripts/ratchet-candidate-ctr.c).
// The source is compiled with the same flags as the image build (35-candidate-launcher.sh); the behavior tests need root
// (to make the binary setuid and to switch uids) and setpriv, so they run in CI containers and on root dev boxes only.

const here = dirname(fileURLToPath(import.meta.url));
const source = join(here, "..", "..", "images", "scripts", "ratchet-candidate-ctr.c");
const have = (c: string) => spawnSync("sh", ["-c", `command -v ${c}`]).status === 0;
const canCompile = have("gcc");
const root = typeof process.getuid === "function" && process.getuid() === 0;
const canRun = canCompile && root && have("setpriv");

const dir = mkdtempSync(join(tmpdir(), "ratchet-launcher-"));
chmodSync(dir, 0o755);
afterAll(() => rmSync(dir, { recursive: true, force: true }));
const bin = join(dir, "cand");
const conf = join(dir, "launcher.conf");
const work = join(dir, "work");
const CALLER = 65534;
const TARGET = 65533;

function compile(flags: string[], out: string): ReturnType<typeof spawnSync> {
  return spawnSync("gcc", ["-O2", "-s", "-Wall", "-Wextra", "-Werror", ...flags, "-o", out, source], { encoding: "utf8" });
}

describe("ratchet-candidate-ctr.c", () => {
  it.skipIf(!canCompile)("compiles statically with the flags of the image build and no warnings", () => {
    const r = compile(["-static"], join(dir, "static"));
    expect(r.stderr).toBe("");
    expect(r.status).toBe(0);
    const ldd = spawnSync("ldd", [join(dir, "static")], { encoding: "utf8" });
    expect(ldd.status).not.toBe(0); // "not a dynamic executable"
  });

  it("states what it refuses in its header, so the docs and the code agree", () => {
    const text = readFileSync(source, "utf8");
    for (const w of ["setuid", "launcher.conf", "no_new_privs", "RATCHET_TRACE", "AWS_", "read-only"]) expect(text).toContain(w);
    expect(text).toContain("/ratchet/launcher.conf");
    expect(text).toContain("PR_SET_NO_NEW_PRIVS");
    expect(text).toContain("setgroups(0, NULL)");
  });

  describe.skipIf(!canRun)("behavior (as root, with setpriv)", () => {
    const setup = (c = `${CALLER} ${TARGET} ${TARGET}\n`) => {
      writeFileSync(conf, c);
      chmodSync(conf, 0o644);
    };
    const asCaller = (args: string[], extraEnv: string[] = [], uid = CALLER) =>
      spawnSync(
        "setpriv",
        [`--reuid=${uid}`, `--regid=${uid}`, "--clear-groups", "env", `RATCHET_CANDIDATE_DIR=${work}`, `RATCHET_CANDIDATE_HOME=${work}`, `RATCHET_CANDIDATE_TMP=${work}`, ...extraEnv, bin, ...args],
        { encoding: "utf8" },
      );

    it("builds, and runs a command as the target uid with no groups, no_new_privs, a clean environment and the checkout as cwd", () => {
      const r = compile(["-static", `-DCONF_PATH="${conf}"`], bin);
      expect(r.status).toBe(0);
      spawnSync("chown", ["root:root", bin]);
      chmodSync(bin, 0o4755);
      mkdirSync(work, { recursive: true });
      chmodSync(work, 0o777);
      setup();
      const out = asCaller(["FOO=bar", "sh", "-c", 'id -u; id -g; pwd; env | sort; grep -E "^(NoNewPrivs|Groups)" /proc/self/status'], ["RATCHET_TRACE=secret", "AWS_SECRET_ACCESS_KEY=x", "OTHER=1"]);
      expect(out.status).toBe(0);
      const lines = out.stdout.split("\n");
      expect(lines.slice(0, 3)).toEqual([String(TARGET), String(TARGET), work]);
      expect(out.stdout).toContain("FOO=bar");
      expect(out.stdout).toContain("USER=ratchet-bench");
      expect(out.stdout).toMatch(/NoNewPrivs:\s*1/);
      expect(out.stdout).toMatch(/Groups:\s*$/m);
      for (const leak of ["RATCHET_TRACE", "AWS_SECRET", "OTHER=1"]) expect(out.stdout).not.toContain(leak);
    });

    it("refuses to pass the holdout or credentials as arguments", () => {
      setup();
      for (const a of ["RATCHET_TRACE=/x", "AWS_SECRET_ACCESS_KEY=x"]) {
        const r = asCaller([a, "true"]);
        expect(r.status).toBe(78);
        expect(r.stderr).toContain("refusing to pass");
      }
    });

    it("only works for the configured caller (real uid), including root", () => {
      setup();
      expect(spawnSync(bin, ["true"], { encoding: "utf8" }).status).toBe(77);
      expect(asCaller(["true"], [], 65000).status).toBe(77);
    });

    it("refuses a configuration that is group-writable, malformed, missing, or names root or the caller as the target", () => {
      setup();
      chmodSync(conf, 0o664);
      expect(asCaller(["true"]).status).toBe(77);
      for (const bad of [`${CALLER} 0 0\n`, `${CALLER} ${CALLER} ${CALLER}\n`, "nonsense\n", `${CALLER} ${TARGET}\n`, `${CALLER} ${TARGET} ${TARGET} extra\n`, ""]) {
        setup(bad);
        expect(asCaller(["true"]).status, JSON.stringify(bad)).toBe(77);
      }
      rmSync(conf);
      expect(asCaller(["true"]).status).toBe(77);
    });

    it("does not let a setuid binary inside the image hand root back to the candidate (no_new_privs)", () => {
      setup();
      const suid = join(dir, "suid-id");
      copyFileSync("/usr/bin/id", suid);
      spawnSync("chown", ["root:root", suid]);
      chmodSync(suid, 0o4755);
      const r = asCaller([suid, "-u"]);
      expect(r.stdout.trim()).toBe(String(TARGET));
    });

    it("keeps the candidate out of files only the evaluator can read", () => {
      setup();
      const secret = join(dir, "holdout.txt");
      writeFileSync(secret, "sealed");
      spawnSync("chown", [`${CALLER}:${CALLER}`, secret]);
      chmodSync(secret, 0o600);
      expect(asCaller(["cat", secret]).status).not.toBe(0);
      expect(spawnSync("setpriv", [`--reuid=${CALLER}`, `--regid=${CALLER}`, "--clear-groups", "cat", secret], { encoding: "utf8" }).stdout).toBe("sealed");
    });

    it("reports a missing command and a missing usage distinctly", () => {
      setup();
      expect(asCaller(["FOO=bar"]).status).toBe(78);
      expect([126, 127]).toContain(asCaller(["no-such-command-xyz"]).status);
    });
  });
});
