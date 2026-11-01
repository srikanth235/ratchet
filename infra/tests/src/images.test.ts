import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// Invariants of the machine-image definitions in infra/images (Packer HCL + provisioning scripts),
// checked without Packer or AWS. The images are what live mode launches (runnerImageRef /
// benchImageRef), and the bench image must agree with the instance user data of crates/ratchet-sandbox
// (`compute/user_data.rs`, `compute/aws.rs`) and the bench scripts of crates/ratchet-bench.

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const imagesDir = join(repo, "infra", "images");
const scriptsDir = join(imagesDir, "scripts");
const read = (...p: string[]) => readFileSync(join(imagesDir, ...p), "utf8");
const readRepo = (...p: string[]) => readFileSync(join(repo, ...p), "utf8");
const stripComments = (s: string) =>
  s
    .split("\n")
    .filter((l) => !/^\s*#/.test(l))
    .join("\n");

const scriptFiles = readdirSync(scriptsDir).filter((f) => f.endsWith(".sh"));
const stepScripts = scriptFiles.filter((f) => /^\d\d-.*\.sh$/.test(f));
const hclFiles = readdirSync(imagesDir).filter((f) => f.endsWith(".pkr.hcl"));

const common = read("common.pkr.hcl");
const runnerHcl = read("runner.pkr.hcl");
const benchHcl = read("bench.pkr.hcl");
const versions = read("scripts", "versions.sh");
const agentVersions = read("scripts", "versions-agents.sh");
const awsRs = readRepo("crates", "ratchet-sandbox", "src", "compute", "aws.rs");
const userDataRs = readRepo("crates", "ratchet-sandbox", "src", "compute", "user_data.rs");
const benchRs = (f: string) => readRepo("crates", "ratchet-bench", "src", f);

/** The string items of `name = [ "a", "b" ]` in a locals block. */
function hclList(hcl: string, name: string): string[] {
  const m = new RegExp(`\\b${name}\\s*=\\s*\\[([^\\]]*)\\]`).exec(hcl);
  if (!m) throw new Error(`no list ${name} in HCL`);
  return [...m[1]!.matchAll(/"([^"]+)"/g)].map((x) => x[1]!);
}

function pins(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const l of text.split("\n")) {
    const m = /^([A-Z0-9_a-z]+)=(\S+)$/.exec(l);
    if (m) out[m[1]!] = m[2]!;
  }
  return out;
}
const allPins = { ...pins(versions), ...pins(agentVersions) };

const runnerScripts = hclList(runnerHcl, "runner_scripts");
const runnerSupport = hclList(runnerHcl, "runner_support_files");
const benchScripts = hclList(benchHcl, "bench_scripts");
/** Added by -var host_toolchain=true (the legacy no-container path). */
const benchHostScripts = hclList(benchHcl, "bench_host_scripts");
const benchSupport = hclList(benchHcl, "bench_support_files");

describe("images: layout", () => {
  it("has a runner and a bench Packer template plus shared definitions", () => {
    expect(hclFiles.sort()).toEqual(["bench.pkr.hcl", "common.pkr.hcl", "runner.pkr.hcl"]);
  });
  it("lists only scripts that exist, and every step script is used by an image", () => {
    const present = readdirSync(scriptsDir);
    for (const f of [...runnerScripts, ...runnerSupport, ...benchScripts, ...benchHostScripts, ...benchSupport]) expect(present, f).toContain(f);
    for (const f of stepScripts) expect([...runnerScripts, ...benchScripts, ...benchHostScripts], `${f} is not used by any image`).toContain(f);
  });
  it("runs the steps in numeric order and ends with verification and cleanup", () => {
    for (const list of [runnerScripts, benchScripts]) {
      expect([...list].sort()).toEqual(list);
      expect(list.slice(-2).map((s) => s.slice(0, 2))).toEqual(["90", "99"]);
    }
    // the bench template merges the optional host-toolchain steps back in by sorting
    expect(benchHcl).toMatch(/bench_run_scripts\s*=\s*sort\(concat\(local\.bench_scripts, var\.host_toolchain \? local\.bench_host_scripts : \[\]\)\)/);
    expect(benchHcl).toMatch(/for f in concat\(local\.bench_support_files, local\.bench_run_scripts\)/);
    expect(benchHcl).toMatch(/for s in local\.bench_run_scripts : "bash/);
  });
  it("shares the base, aws cli and cleanup steps between both images; node and the load tools are the runner's (the bench only with host_toolchain)", () => {
    for (const s of ["00-base.sh", "20-aws-cli.sh", "99-cleanup.sh"]) {
      expect(runnerScripts).toContain(s);
      expect(benchScripts).toContain(s);
    }
    for (const s of ["10-node.sh", "30-bench-tools.sh"]) {
      expect(runnerScripts).toContain(s);
      expect(benchScripts).not.toContain(s);
      expect(benchHostScripts).toContain(s);
    }
    expect(common).toMatch(/variable "host_toolchain"[\s\S]*?default\s*=\s*false/);
  });
});

describe("images: script hygiene", () => {
  it.each(stepScripts)("%s is an executable bash script with strict mode", (f) => {
    const text = read("scripts", f);
    expect(text.startsWith("#!/bin/bash\n")).toBe(true);
    expect(text).toMatch(/^set -euo pipefail$/m);
    expect(statSync(join(scriptsDir, f)).mode & 0o111, `${f} should be executable`).not.toBe(0);
  });
  it.each([...scriptFiles.map((f) => join("scripts", f)), "build.sh", "pack-runner.sh"])("%s parses (bash -n)", (f) => {
    const r = spawnSync("bash", ["-n", join(imagesDir, f)], { encoding: "utf8" });
    expect(r.stderr).toBe("");
    expect(r.status).toBe(0);
  });
  const shellcheck = spawnSync("shellcheck", ["--version"]).status === 0;
  it.skipIf(!shellcheck)("passes shellcheck", () => {
    const files = [...scriptFiles.map((f) => join("scripts", f)), "build.sh", "pack-runner.sh"];
    const r = spawnSync("shellcheck", ["-x", "-P", "SCRIPTDIR", "-s", "bash", ...files], { cwd: imagesDir, encoding: "utf8" });
    expect(r.stdout).toBe("");
    expect(r.status).toBe(0);
  }, 60_000);
  it("only uses https downloads, never pipes a download into a shell and never skips TLS checks", () => {
    for (const f of [...scriptFiles.map((x) => join("scripts", x)), "build.sh", "pack-runner.sh", ...hclFiles]) {
      const text = stripComments(read(f));
      expect(text, f).not.toMatch(/http:\/\//);
      expect(text, f).not.toMatch(/\|\s*(sudo\s+)?(ba|z)?sh\b/);
      expect(text, f).not.toMatch(/(--insecure|curl\s+[^\n]*\s-k\b|--no-check-certificate|NODE_TLS_REJECT_UNAUTHORIZED)/);
    }
  });
  it("bakes no credentials or model keys into any image file", () => {
    const files = [...scriptFiles.map((x) => join("scripts", x)), "build.sh", "pack-runner.sh", ...hclFiles];
    for (const f of files) {
      const text = read(f);
      expect(text, f).not.toMatch(/sk-ant-|sk-proj-|\bsk-[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{20,}|github_pat_|-----BEGIN [A-Z ]*PRIVATE KEY/);
      expect(text, f).not.toMatch(/(ANTHROPIC|OPENAI|CODEX)_API_KEY\s*=\s*["']?[^\s"'$]/);
    }
  });
});

describe("images: pinned versions and checksums", () => {
  const versionKeys = Object.keys(allPins).filter((k) => k.endsWith("_VERSION"));
  it("pins every tool to an exact x.y.z version", () => {
    expect(versionKeys.sort()).toEqual(["AWSCLI_VERSION", "CLAUDE_CODE_VERSION", "CODEX_VERSION", "HYPERFINE_VERSION", "K6_VERSION", "NODE_VERSION", "PNPM_VERSION"]);
    for (const k of versionKeys) expect(allPins[k], k).toMatch(/^\d+\.\d+\.\d+$/);
  });
  it("installs Node 22 and the pnpm the repository declares", () => {
    expect(allPins["NODE_VERSION"]).toMatch(/^22\./);
    const pm = (JSON.parse(readRepo("package.json")) as { packageManager: string }).packageManager;
    expect(pm).toBe(`pnpm@${allPins["PNPM_VERSION"]}`);
  });
  it("has a SHA-256 for every archive and both architectures", () => {
    const want: Record<string, string[]> = {
      NODE: ["x64", "arm64"],
      AWSCLI: ["x86_64", "aarch64"],
      K6: ["amd64", "arm64"],
      HYPERFINE: ["x86_64", "aarch64"],
    };
    for (const [tool, arches] of Object.entries(want)) {
      for (const a of arches) expect(allPins[`${tool}_SHA256_${a}`], `${tool} ${a}`).toMatch(/^[0-9a-f]{64}$/);
    }
    const sums = Object.entries(allPins).filter(([k]) => k.includes("_SHA256_"));
    expect(new Set(sums.map(([, v]) => v)).size, "checksums must be distinct").toBe(sums.length);
  });
  it("has the registry SHA-512 integrity of every npm tarball", () => {
    for (const k of ["PNPM_INTEGRITY", "CLAUDE_CODE_INTEGRITY", "CODEX_INTEGRITY"]) expect(allPins[k], k).toMatch(/^sha512-[A-Za-z0-9+/]{86}==$/);
  });
  it("verifies every download before use", () => {
    for (const f of stepScripts) {
      const text = stripComments(read("scripts", f));
      expect(text, `${f} must not call curl/wget directly`).not.toMatch(/\b(curl|wget)\b/);
      expect(text, `${f} must not use the unverified fetch`).not.toMatch(/(^|[^_a-z])fetch\s+["']?http/m);
    }
    const lib = stripComments(read("scripts", "lib.sh"));
    expect(lib).toMatch(/fetch_verified\(\)\s*\{\s*fetch "\$1" "\$2"\s*verify_sha256 "\$2" "\$3"/);
    expect(lib).toMatch(/verify_sri "\$WORK\/\$out" "\$integrity"/);
  });
  it("installs npm packages only from verified tarballs at exact versions", () => {
    for (const f of stepScripts) {
      const text = stripComments(read("scripts", f));
      for (const line of text.split("\n").filter((l) => /\bnpm (install|i)\b/.test(l))) {
        if (f === "60-ratchet-runner.sh") continue; // packed local tarballs, covered by SHA256SUMS below
        expect(line, `${f}: ${line}`).toMatch(/\$\{?[a-z_]*_tgz\}?"?\s*$/);
      }
      expect(text, f).not.toMatch(/@(latest|next|beta)\b/);
      expect(text, f).not.toMatch(/npm (install|i)\s[^\n]*@[\^~]/);
    }
  });
  it("pins the Packer plugin and requires a Packer version", () => {
    expect(common).toMatch(/version\s*=\s*"= \d+\.\d+\.\d+"/);
    expect(common).toMatch(/required_version\s*=\s*">= \d+\.\d+/);
    expect(common).toMatch(/github\.com\/hashicorp\/amazon/);
  });
});

describe("images: AMI tags", () => {
  it("tags the runner AMI ratchet:image=runner and the bench AMI ratchet:image=bench, with a version", () => {
    expect(runnerHcl).toMatch(/"ratchet:image"\s*=\s*"runner"/);
    // the generic image is the one the tag lookup finds; the host-toolchain variant has its own tag so it is never picked
    expect(benchHcl).toMatch(/"ratchet:image"\s*=\s*var\.host_toolchain \? "bench-host" : "bench"/);
    expect(runnerHcl).not.toMatch(/"ratchet:image"\s*=\s*"bench"/);
    expect(benchHcl).not.toMatch(/"ratchet:image"\s*=\s*"runner"/);
    expect(common).toMatch(/"ratchet:image_version"\s*=\s*var\.image_version/);
    expect(common).toMatch(/variable "image_version"/);
    expect(common).toMatch(/"ratchet:source_commit"/);
  });
  it("applies the tags to the AMI and its snapshots, and names the AMIs by version", () => {
    for (const [hcl, kind] of [
      [runnerHcl, "runner"],
      [benchHcl, "bench"],
    ] as const) {
      expect(hcl).toMatch(new RegExp(`\\btags\\s*=\\s*merge\\(local\\.${kind}_tags`));
      expect(hcl).toMatch(new RegExp(`snapshot_tags\\s*=\\s*local\\.${kind}_tags`));
      expect(hcl).toMatch(new RegExp(`ami_name\\s*=\\s*local\\.${kind}_name`));
      expect(hcl).toMatch(
        new RegExp(`${kind}_name\\s*=\\s*"ratchet-${kind}(\\$\\{var\\.host_toolchain \\? "-host" : ""\\})?-\\$\\{var\\.image_version\\}-\\$\\{var\\.arch\\}"`),
      );
    }
  });
  it("does not let extra_tags override the reserved ratchet:* tags", () => {
    expect(common).toMatch(/merge\(var\.extra_tags, \{/);
    expect(runnerHcl).toMatch(/runner_tags = merge\(local\.common_tags, \{/);
  });
  it("names its builds runner and bench, as build.sh selects them", () => {
    expect(runnerHcl).toMatch(/build \{\s*name\s*=\s*"runner"/);
    expect(benchHcl).toMatch(/build \{\s*name\s*=\s*"bench"/);
    expect(read("build.sh")).toContain('-only="${name}.*"');
  });
  it("keeps the builder and the launched instances on IMDSv2, matching the launch parameters", () => {
    for (const hcl of [runnerHcl, benchHcl]) {
      expect(hcl).toMatch(/imds_support\s*=\s*"v2\.0"/);
      expect(hcl).toMatch(/http_tokens\s*=\s*"required"/);
      expect(hcl).toMatch(/http_put_response_hop_limit\s*=\s*1/);
    }
    expect(awsRs).toContain(".http_tokens(ec2t::HttpTokensState::Required)");
    expect(awsRs).toContain(".http_put_response_hop_limit(1)");
  });
  it("builds on Amazon Linux 2023 from the amazon account", () => {
    for (const hcl of [runnerHcl, benchHcl]) {
      expect(hcl).toMatch(/al2023-ami-2023\.\*-\$\{var\.arch\}/);
      expect(hcl).toMatch(/owners\s*=\s*\["amazon"\]/);
      expect(hcl).toMatch(/ssh_username\s*=\s*"ec2-user"/);
    }
  });
});

describe("bench image", () => {
  const benchFiles = [...benchSupport, ...benchScripts];
  it("never installs the agent CLIs, their pins or the runner", () => {
    for (const forbidden of ["40-runner-tools.sh", "50-agent-clis.sh", "60-ratchet-runner.sh", "90-verify-runner.sh", "versions-agents.sh"]) {
      expect(benchFiles).not.toContain(forbidden);
    }
    const text = stripComments(benchHcl);
    expect(text).not.toMatch(/claude|codex|anthropic|openai|versions-agents|dist\/|ratchet-runner/i);
  });
  it("contains no mention of the agent CLIs or model keys in what it provisions (except the negative check)", () => {
    for (const f of benchFiles.filter((x) => x !== "90-verify-bench.sh")) {
      expect(stripComments(read("scripts", f)), f).not.toMatch(/claude|codex|anthropic|openai|model.?key|API_KEY/i);
    }
    const verify = stripComments(read("scripts", "90-verify-bench.sh"));
    expect(verify).toContain('die "$c must not be on the bench image"');
    expect(verify).toMatch(/for c in claude codex ratchet-runner/);
    expect(verify).toMatch(/ANTHROPIC_\* \| OPENAI_\* \| CODEX_\*\) die/);
  });
  it("is generic: git, the AWS CLI, the SSM agent, zstd and the Docker engine; node, pnpm, k6 and hyperfine only with host_toolchain", () => {
    expect(stripComments(read("scripts", "00-base.sh"))).toMatch(/\bgit\b[\s\S]*amazon-ssm-agent/);
    expect(stripComments(read("scripts", "00-base.sh"))).toContain("systemctl enable amazon-ssm-agent");
    expect(stripComments(read("scripts", "00-base.sh"))).toMatch(/\bzstd\b/);
    expect(benchScripts).toContain("25-docker.sh");
    expect(benchScripts).not.toContain("10-node.sh");
    const verify = read("scripts", "90-verify-bench.sh");
    expect(verify).toContain('if [ "$HOST_TOOLCHAIN" = 1 ]');
    expect(verify).toContain("must not be on the generic bench image");
    expect(benchHcl).toContain("RATCHET_HOST_TOOLCHAIN=");
    expect(read("scripts", "10-node.sh")).toContain("pnpm");
    expect(read("scripts", "30-bench-tools.sh")).toMatch(/k6[\s\S]*hyperfine/);
  });
  it("installs Docker without opening it up: no group members, no tcp socket, no seccomp or no-new-privileges override, no pulled images", () => {
    const docker = stripComments(read("scripts", "25-docker.sh"));
    expect(docker).toContain("dnf_install docker");
    expect(docker).toContain("systemctl enable docker");
    expect(docker).not.toMatch(/usermod|gpasswd|groupadd|--group|docker\s+(pull|run|build)/);
    const daemon = /cat > "\$ETC\/docker\/daemon.json" <<'EOF'\n([\s\S]*?)\nEOF/.exec(docker)![1]!;
    const cfg = JSON.parse(daemon) as Record<string, unknown>;
    expect(cfg["icc"]).toBe(false);
    expect(cfg["userland-proxy"]).toBe(false);
    for (const k of ["hosts", "tls", "tlsverify", "seccomp-profile", "no-new-privileges", "privileged", "userns-remap", "insecure-registries"]) expect(Object.keys(cfg), k).not.toContain(k);
    const verify = stripComments(read("scripts", "90-verify-bench.sh"));
    expect(verify).toContain("the docker group must have no members");
    expect(verify).toContain("the docker daemon runs without its default seccomp profile");
    expect(verify).toContain("docker-init");
    expect(stripComments(read("scripts", "99-cleanup.sh"))).toContain("rm -rf /var/lib/docker");
  });
  it("installs the prebuilt Rust launcher (sha256-checked, setuid root, root-only directory); nothing is compiled on the image", () => {
    const script = stripComments(read("scripts", "35-candidate-launcher.sh"));
    expect(script).toContain('src="${RATCHET_LAUNCHER_BINARY:-$SCRIPT_DIR/ratchet-candidate-launcher}"');
    expect(script).toContain('want="${RATCHET_LAUNCHER_SHA256:-}"');
    expect(script).toContain('verify_sha256 "$bin" "$want"');
    expect(script).toContain('elf_is_native "$bin"');
    expect(script).toContain('elf_is_static "$bin"');
    expect(script).toContain('install -d -m 0700 -o root -g root "$dest"');
    expect(script).toContain('install -m 4755 -o root -g root "$bin" "$dest/ratchet-candidate-ctr"');
    expect(script).not.toMatch(/\bgcc\b|glibc-static|dnf|\bcargo\b|rustc/);
    // the digest is checked on the copy that is installed, and before anything is installed
    expect(script.indexOf("verify_sha256")).toBeGreaterThan(script.indexOf('cp -- "$src" "$bin"'));
    expect(script.indexOf("verify_sha256")).toBeLessThan(script.indexOf("install -d"));
    // the path and name the bench scripts mount into the container
    const rs = benchRs("image_scripts.rs");
    expect(/CANDIDATE_HELPER: &str = "([^"]+)"/.exec(rs)![1]).toBe("/usr/local/libexec/ratchet/ratchet-candidate-ctr");
    expect(script).toContain('dest="$PREFIX/libexec/ratchet"');
    // the C program and its compile step are gone
    expect(existsSync(join(scriptsDir, "ratchet-candidate-ctr.c"))).toBe(false);
    expect(benchSupport).not.toContain("ratchet-candidate-ctr.c");
    const verify = stripComments(read("scripts", "90-verify-bench.sh"));
    expect(verify).toContain("root:root 4755");
    expect(verify).toContain("root:root 700");
    expect(verify).toContain("must be statically linked");
    expect(verify).toContain("elf_is_static");
    expect(verify).not.toMatch(/\bldd\b.*\|\|/); // ldd exits 0 for a static-pie executable
    expect(verify).toContain("must exit 77");
    expect(verify).toMatch(/for c in gcc cc make/);
  });
  it("uploads the launcher and its SHA-256 to the bench builder only, from the CI artifact", () => {
    // a file provisioner puts it next to the scripts, where 35-candidate-launcher.sh looks by default
    expect(benchHcl).toMatch(/provisioner "file" \{\s*source\s*=\s*local\.launcher_path\s*destination\s*=\s*"\$\{local\.remote_dir\}\/ratchet-candidate-launcher"/);
    expect(benchHcl).toContain("RATCHET_LAUNCHER_SHA256=${var.launcher_sha256}");
    expect(common).toMatch(/variable "launcher_binary"[\s\S]*?default\s*=\s*""/);
    expect(common).toMatch(/variable "launcher_sha256"[\s\S]*?\[0-9a-fA-F\]\{64\}/);
    expect(common).toContain("launcher/ratchet-candidate-launcher-${var.arch}");
    expect(stripComments(runnerHcl)).not.toMatch(/launcher/);
    // build.sh checks the digest before a builder is launched and hands both variables to Packer
    const build = read("build.sh");
    expect(build).toContain("sha256 mismatch");
    expect(build).toContain('-var "launcher_binary=$bin" -var "launcher_sha256=$got"');
    expect(readRepo(".gitignore")).toContain("/infra/images/launcher/");
  });
  it("builds the launcher in CI as a static binary per architecture, records its SHA-256 and uploads it", () => {
    const wf = readRepo(".github", "workflows", "images.yml");
    expect(wf).toContain("RUSTFLAGS: -C target-feature=+crt-static");
    expect(wf).toContain("--bin ratchet-candidate-launcher");
    for (const t of ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "ubuntu-24.04-arm"]) expect(wf, t).toContain(t);
    expect(wf).toContain("sha256sum");
    expect(wf).toContain("actions/upload-artifact@v4");
    expect(wf).toContain("name: ratchet-candidate-launcher-${{ matrix.arch }}");
    // the artifact names are the ones launcher_path / build.sh look for (Packer arch names)
    expect(wf).toMatch(/arch: x86_64/);
    expect(wf).toMatch(/arch: arm64/);
    expect(wf).toContain("infra/images/scripts/35-candidate-launcher.sh");
  });
  it("provides what remote_scripts.rs needs: setpriv, pkill, GNU grep -P, find -printf, iptables and ip6tables", () => {
    const remote = benchRs("remote_scripts.rs");
    for (const c of ["setpriv", "iptables", "pkill", "sha256sum"]) expect(remote).toContain(c);
    const verify = read("scripts", "90-verify-bench.sh");
    for (const c of ["setpriv", "pkill", "iptables", "ip6tables", "sha256sum", "aws", "node", "pnpm", "k6"]) expect(verify).toMatch(new RegExp(`\\b${c}\\b`));
    expect(verify).toContain("grep -oP");
    expect(verify).toContain("-printf");
    expect(stripComments(read("scripts", "00-base.sh"))).toMatch(/util-linux/);
    expect(stripComments(read("scripts", "00-base.sh"))).toMatch(/procps-ng/);
    expect(stripComments(read("scripts", "70-bench-user.sh"))).toMatch(/dnf_install iptables-nft/);
  });
  it("provides what the dependency-snapshot job and restore need: zstd (gzip is the fallback), tar --sort, node, pnpm and npm for the unprivileged user", () => {
    const deps = benchRs("deps_scripts.rs") + benchRs("remote_scripts.rs");
    expect(deps).toContain("zstd");
    expect(deps).toContain("--sort=name");
    expect(stripComments(read("scripts", "00-base.sh"))).toMatch(/\bzstd\b/);
    const verify = read("scripts", "90-verify-bench.sh");
    expect(verify).toMatch(/\bzstd\b/);
    expect(verify).toMatch(/\bnpm\b/);
    // the snapshot job needs no extra OS user on the image: its user data creates `ratchet-deps` like the bench users
    expect(stripComments(read("scripts", "70-bench-user.sh"))).not.toContain("ratchet-deps");
  });
  it("creates the ratchet-bench and ratchet-holdout users exactly like the instance user data does", () => {
    const name = /BENCH_OS_USER: &str = "([^"]+)"/.exec(userDataRs)![1]!;
    expect(name).toBe("ratchet-bench");
    const holdout = /HOLDOUT_OS_USER: &str = "([^"]+)"/.exec(userDataRs)![1]!;
    expect(holdout).toBe("ratchet-holdout");
    expect(/BENCH_USER: &str = "([^"]+)"/.exec(benchRs("shell.rs"))![1]).toBe(name);
    // The user data loops over both users: `id -u ${u} ... || useradd <flags> ${u}`.
    const userData = /useradd ([^"\\]*?)\{u\}/.exec(userDataRs)![1]!.trim();
    expect(userData).toBe("--system --no-create-home --shell /usr/sbin/nologin");
    const script = stripComments(read("scripts", "70-bench-user.sh"));
    expect(script).toContain(`BENCH_USER=${name}`);
    expect(script).toContain(`HOLDOUT_USER=${holdout}`);
    expect(script).toContain('for u in "$BENCH_USER" "$HOLDOUT_USER"; do');
    expect(script).toContain(`useradd ${userData} "$u"`);
    expect(script).toMatch(/id -u "\$u" >\/dev\/null 2>&1 \|\| useradd/); // same idempotent guard as the user data
  });
  it("leaves the IMDS block to the user data: no persistent rule in the image, none left behind", () => {
    const rule = /iptables -I OUTPUT 1 (-m owner --uid-owner \{u\} -d 169\.254\.169\.254 -j REJECT)/.exec(userDataRs)![1]!.replace("{u}", "ratchet-bench");
    // Nothing in the provisioning steps inserts or saves firewall rules...
    for (const f of benchScripts.filter((x) => x !== "90-verify-bench.sh")) {
      const text = stripComments(read("scripts", f));
      expect(text, f).not.toMatch(/\bip6?tables\s+(-I|-A|--insert|--append)\b/);
      expect(text, f).not.toMatch(/iptables-(save|restore)|iptables-services|netfilter-persistent|firewalld/);
    }
    // ...the verify step proves the kernel accepts the very same rule, then removes it again.
    const verify = stripComments(read("scripts", "90-verify-bench.sh"));
    const arr = /IMDS_RULE=\(([^)]*)\)/.exec(verify)![1]!.replace("OUTPUT ", "").replace('"$BU"', "ratchet-bench");
    expect(arr).toBe(rule);
    expect(verify).toMatch(/iptables -I "\$\{IMDS_RULE\[@\]\}"/);
    expect(verify).toMatch(/iptables -D "\$\{IMDS_RULE\[@\]\}"/);
    expect(verify).toContain("the image must not keep the IMDS rule");
    // And the user data still carries the rule + the IPv6 twin the image relies on.
    expect(userDataRs).toContain("ip6tables -I OUTPUT 1 -m owner --uid-owner {u} -d fd00:ec2::254 -j REJECT");
  });
  it("probes the toolchain as the unprivileged user with the same setpriv flags as the bench runner", () => {
    const remote = benchRs("remote_scripts.rs");
    const flags = '--reuid="$BU" --regid="$BU" --clear-groups --no-new-privs';
    expect(remote).toContain(`setpriv ${flags}`);
    expect(read("scripts", "90-verify-bench.sh")).toContain(`setpriv ${flags}`);
  });
  it("keeps the user out of sudo/wheel and gives it no home directory", () => {
    const script = read("scripts", "70-bench-user.sh");
    expect(script).toMatch(/for g in wheel sudo/);
    expect(read("scripts", "90-verify-bench.sh")).toContain('should have no home directory');
  });
  it("installs everything world-readable, never under /root", () => {
    for (const f of stepScripts.filter((x) => x !== "99-cleanup.sh")) expect(stripComments(read("scripts", f)), f).not.toMatch(/\/root\//); // cleanup only deletes caches there
    expect(read("scripts", "lib.sh")).toMatch(/make_world_readable\(\) \{ chmod -R a\+rX/);
    expect(read("scripts", "10-node.sh")).toContain('make_world_readable "$node_dir"');
  });
});

describe("runner image", () => {
  it("installs the tools the issue asks for", () => {
    const text = (f: string) => stripComments(read("scripts", f));
    expect(text("00-base.sh")).toMatch(/\bgit\b/);
    expect(text("00-base.sh")).toContain("amazon-ssm-agent");
    expect(text("10-node.sh")).toContain("node-v${NODE_VERSION}");
    expect(text("20-aws-cli.sh")).toContain("awscli-exe-linux");
    expect(text("30-bench-tools.sh")).toMatch(/k6[\s\S]*hyperfine/);
    expect(text("40-runner-tools.sh")).toMatch(/\bperf\b/);
  });
  it("installs the claude and codex CLIs at the pinned, integrity-checked versions", () => {
    const s = stripComments(read("scripts", "50-agent-clis.sh"));
    expect(s).toContain('npm_fetch_verified @anthropic-ai/claude-code "$CLAUDE_CODE_VERSION" "$CLAUDE_CODE_INTEGRITY"');
    expect(s).toContain('npm_fetch_verified @openai/codex "$CODEX_VERSION" "$CODEX_INTEGRITY"');
    expect(s).toContain("versions-agents.sh");
    expect(runnerSupport).toContain("versions-agents.sh");
  });
  it("keeps the pinned claude version from updating itself", () => {
    expect(stripComments(read("scripts", "50-agent-clis.sh"))).toContain('"DISABLE_AUTOUPDATER": "1"');
  });
  it("installs the ratchet-runner bin the SSM launcher starts, from a verified prebuilt static binary", () => {
    const cargo = readRepo("crates", "ratchet-agents", "Cargo.toml");
    const bins = [...cargo.matchAll(/\[\[bin\]\]\s*name\s*=\s*"([^"]+)"/g)].map((m) => m[1]);
    expect(bins).toEqual(["ratchet-runner"]);
    const ssm = readRepo("crates", "ratchet-agents", "src", "control", "ssm.rs");
    expect(ssm).toContain('"ratchet-runner"');
    const s = stripComments(read("scripts", "60-ratchet-runner.sh"));
    expect(s).toContain("sha256sum --check --strict SHA256SUMS");
    expect(s).toContain('install -m 0755 "$dist/ratchet-runner" "$app/ratchet-runner"');
    expect(s).toContain('"$PREFIX/bin/ratchet-runner"');
    expect(s).not.toMatch(/\bnpm\b|\bpnpm\b|node_modules|tarball/);
    const pack = stripComments(read("pack-runner.sh"));
    expect(pack).toContain("cargo build --release -p ratchet-agents --bin ratchet-runner");
    expect(pack).toContain("x86_64-unknown-linux-musl");
    expect(pack).toContain("aarch64-unknown-linux-musl");
    expect(pack).toContain("SHA256SUMS");
    expect(pack).not.toMatch(/\bpnpm\b/);
    expect(runnerHcl).toMatch(/source\s*=\s*"\$\{path\.root\}\/dist\/"/);
    expect(read(".gitignore")).toContain("dist/");
  });
  it("builds the runner in CI as a static musl binary per architecture and installs it like the image build does", () => {
    const wf = readRepo(".github", "workflows", "images.yml");
    expect(wf).toContain("--bin ratchet-runner");
    for (const t of ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"]) expect(wf, t).toContain(t);
    expect(wf).toContain("name: ratchet-runner-${{ matrix.arch }}");
    expect(wf).toContain("infra/images/pack-runner.sh");
    expect(wf).toContain("infra/images/scripts/60-ratchet-runner.sh");
  });
  it("loads the launch environment the user data writes, without letting it override the caller", () => {
    expect(userDataRs).toContain("/etc/ratchet/runner.env");
    const s = stripComments(read("scripts", "60-ratchet-runner.sh"));
    expect(s).toContain("env_file=/etc/ratchet/runner.env");
    expect(s).toContain('saved="$(export -p)"');
    expect(s).toMatch(/eval "\$saved"/);
  });
  it("does not start a second runner from user data: the loop is started over SSM only", () => {
    expect(awsRs + userDataRs).not.toMatch(/nohup ratchet/);
    expect(userDataRs).toContain("touch /run/ratchet/user-data.done");
    for (const f of runnerScripts) expect(stripComments(read("scripts", f)), f).not.toMatch(/\/usr\/local\/bin\/ratchet\b(?!-)/);
  });
  it("is not the bench image: no ratchet-bench user and no firewall changes", () => {
    for (const f of runnerScripts.filter((x) => !x.startsWith("90-"))) {
      const text = stripComments(read("scripts", f));
      expect(text, f).not.toMatch(/useradd|iptables/);
    }
    expect(runnerScripts).not.toContain("70-bench-user.sh");
  });
  it("verifies the result at build time and leaves no model keys, state or launch environment in the AMI", () => {
    const v = stripComments(read("scripts", "90-verify-runner.sh"));
    for (const c of ["claude", "codex", "ratchet-runner", "aws", "k6", "hyperfine", "perf"]) expect(v).toMatch(new RegExp(`\\b${c}\\b`));
    expect(v).toContain("/etc/ratchet/runner.env must come from user data");
    expect(v).toMatch(/ANTHROPIC_\* \| OPENAI_\* \| CODEX_\*\) die/);
    const c = stripComments(read("scripts", "99-cleanup.sh"));
    expect(c).toContain("cloud-init clean");
    expect(c).toContain("authorized_keys");
    expect(c).toContain("/tmp/ratchet-image");
  });
});

describe("images: documentation and wiring", () => {
  it("documents the two AMI ids as runnerImageRef / benchImageRef in the AWS and agents guides", () => {
    for (const f of ["aws.md", "agents.md"]) {
      const doc = readRepo("docs", "real-env", f);
      expect(doc, f).toContain("Build images");
      expect(doc, f).toContain("infra/images");
      expect(doc, f).toContain("runnerImageRef");
      expect(doc, f).toContain("benchImageRef");
    }
    const live = readRepo("crates", "ratchet-engine", "src", "runtime", "live.rs");
    expect(live).toMatch(/runner_image_ref: Option<String>/);
    expect(live).toMatch(/bench_image_ref: Option<String>/);
  });
  it("has a README with the build procedure and the EC2 Image Builder alternative", () => {
    expect(existsSync(join(imagesDir, "README.md"))).toBe(true);
    const readme = read("README.md");
    expect(readme).toContain("EC2 Image Builder");
    expect(readme).toContain("packer init");
    expect(readme).toContain("ratchet:image");
    expect(readme).toContain("Bumping a pin");
  });
});
