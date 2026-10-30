import { BENCH_OS_USER, CANDIDATE_LAUNCHER_PATH, DEPS_OS_USER, HOLDOUT_OS_USER, benchUserData, depsUserData } from '@ratchet/aws';
import { BENCH_USER, CANDIDATE_LAUNCHER, DEPS_BASE_ARCHIVE, DEPS_PREFIX, DEPS_USER, HOLDOUT_USER, STAGING_PREFIX } from '@ratchet/bench';
import { describe, expect, it } from 'vitest';

// The bench scripts (packages/bench) and the instance user-data (packages/aws) are written by different
// packages but must agree on the OS users and the launcher path: a mismatch would make every live job fail
// its isolation check, or worse, be checked against the wrong names.
describe('bench instance contract between @ratchet/aws and @ratchet/bench', () => {
  it('agree on the candidate user, the holdout user and the launcher path', () => {
    expect(BENCH_OS_USER).toBe(BENCH_USER);
    expect(HOLDOUT_OS_USER).toBe(HOLDOUT_USER);
    expect(CANDIDATE_LAUNCHER_PATH).toBe(CANDIDATE_LAUNCHER);
  });

  it('the user-data of a confirm instance sets up every user and the launcher the scripts check for', () => {
    const ud = benchUserData({ optimizationId: 'opt1', instanceType: 'c7i.2xlarge', imageRef: 'ami', purpose: 'confirm' }, 2);
    expect(ud).toContain(`useradd --system --no-create-home --shell /usr/sbin/nologin ${BENCH_USER}`);
    expect(ud).toContain(`useradd --system --no-create-home --shell /usr/sbin/nologin ${HOLDOUT_USER}`);
    expect(ud).toContain(`install -D -m 755 -o root -g root /dev/null ${CANDIDATE_LAUNCHER}`);
    for (const u of [BENCH_USER, HOLDOUT_USER]) expect(ud).toContain(`--uid-owner ${u} -d 169.254.169.254 -j REJECT`);
  });

  it('agree on the dependency-snapshot user, and the user-data of a deps instance sets it up and blocks metadata for it', () => {
    expect(DEPS_OS_USER).toBe(DEPS_USER);
    const ud = depsUserData({ optimizationId: 'opt1', instanceType: 'c7i.2xlarge', imageRef: 'ami', purpose: 'deps' }, 2);
    expect(ud).toContain(`useradd --system --no-create-home --shell /usr/sbin/nologin ${DEPS_USER}`);
    expect(ud).toContain(`--uid-owner ${DEPS_USER} -d 169.254.169.254 -j REJECT`);
  });

  it('agree on the prefixes the deps identity is scoped to in infra/aws (reads bench-inputs/*/deps-base.tgz, writes bench-deps/*)', async () => {
    const { readFileSync } = await import('node:fs');
    const policy = readFileSync(new URL('../../../infra/aws/policies/deps_runner_policy.json.tftpl', import.meta.url), 'utf8');
    expect(policy).toContain(`/${STAGING_PREFIX}*/${DEPS_BASE_ARCHIVE}`);
    expect(policy).toContain(`/${DEPS_PREFIX}*`);
  });
});
