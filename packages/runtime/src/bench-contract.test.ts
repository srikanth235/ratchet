import { BENCH_OS_USER, CANDIDATE_LAUNCHER_PATH, HOLDOUT_OS_USER, benchUserData } from '@ratchet/aws';
import { BENCH_USER, CANDIDATE_LAUNCHER, HOLDOUT_USER } from '@ratchet/bench';
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
});
