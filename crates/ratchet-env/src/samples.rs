//! Deterministic example environments, one per scope (`samples.ts`): what the mock drafter "drafts" (simulation, the
//! desktop's demo mode, e2e) and what the tests feed the validator. They are real drafts: every one passes
//! `validate_environment` and, for `full`, the base module's own validations (`infra/modules/sandbox`).

use ratchet_core::config::DEFAULT_ENVIRONMENT_INFRA_DIR as D;
use ratchet_core::enums::EnvironmentScope;

use crate::config_yaml::{EnvironmentConfigPatch, apply_environment_to_config_yaml};
use crate::types::EnvironmentFile;

pub const MOCK_CONFIG_YAML: &str = "version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
evaluator:
  template: http-latency
";

const FULL_MAIN_TF: &str = r#"# Drafted by Ratchet's setup agent from the repo and the goal. Locked once approved: agents change code only.
#
# The API under test, Postgres with seeded orders, Redis and a separate load generator, all on the sandbox's private network
# (no route to the internet) with the evaluation-only identity, on pinned hardware.

module "sandbox" {
  source  = "./ratchet-sandbox"
  ratchet = var.ratchet

  machines = {
    api = {
      role          = "app"
      label         = "API"
      detail        = "shop-api in the repo's container"
      instance_type = "c7i.2xlarge"
      user_data     = templatefile("${path.module}/scripts/api.sh", { artifacts_bucket = var.ratchet.artifacts_bucket })
    }
    postgres = {
      role           = "database"
      label          = "Postgres 16"
      detail         = "2.1 GB of anonymised orders"
      instance_type  = "r7g.large"
      data_volume_gb = 20
      user_data      = templatefile("${path.module}/seed.sh", { artifacts_bucket = var.ratchet.artifacts_bucket })
    }
    redis = {
      role          = "cache"
      label         = "Redis 7"
      detail        = "emptied before each measurement"
      instance_type = "t4g.medium"
      user_data     = file("${path.module}/scripts/redis.sh")
    }
    load = {
      role          = "load"
      label         = "Load generator"
      detail        = "k6, on its own host"
      instance_type = "c7i.large"
      user_data     = file("${path.module}/scripts/load.sh")
    }
  }
}

# Synthetic seed data the database host loads at start-up (the host has no internet; it reads it from the sandbox's store).
resource "aws_s3_object" "seed_orders" {
  bucket  = module.sandbox.artifacts_bucket
  key     = "env/seed/orders.sql"
  content = file("${path.module}/seed/orders.sql")
  tags    = module.sandbox.tags
}
"#;

const FULL_DOCKERFILE: &str = "# The service under test, built from the protected ref by Ratchet's dependency job (which has internet) and loaded offline.
FROM node:22-slim
WORKDIR /app
COPY . .
CMD [\"node\", \"dist/server.js\"]
";

const FULL_SEED_SH: &str = r#"#!/bin/bash
# Database host: load the synthetic orders from the sandbox's store, then start Postgres.
set -euo pipefail
aws s3 cp "s3://${artifacts_bucket}/env/seed/orders.sql" /var/lib/ratchet/orders.sql
docker load -i /opt/ratchet/images/postgres-16.tar
docker run -d --name db -p 5432:5432 -v /data:/var/lib/postgresql/data postgres:16
"#;

const FULL_ORDERS_SQL: &str = "-- synthetic, anonymised orders
CREATE TABLE IF NOT EXISTS orders (id serial PRIMARY KEY, customer int NOT NULL, total_cents int NOT NULL);
INSERT INTO orders (customer, total_cents) SELECT (i % 5000), (i * 37) % 100000 FROM generate_series(1, 100000) AS i;
";

const FULL_API_SH: &str = "#!/bin/bash
# API host: start the repo's container, pointed at the database and cache on the private network.
set -euo pipefail
docker load -i /opt/ratchet/images/repo.tar
docker run -d --name api -p 8080:8080 repo:ratchet
";

const FULL_REDIS_SH: &str = "#!/bin/bash
set -euo pipefail
docker load -i /opt/ratchet/images/redis-7.tar
docker run -d --name cache -p 6379:6379 redis:7
";

const FULL_LOAD_SH: &str = "#!/bin/bash
# Load generator: idle until the evaluator starts k6 against the API (it never runs candidate code).
set -euo pipefail
sleep infinity
";

const SINGLE_DOCKERFILE: &str =
    "# The toolchain the evaluator runs the repo in, built from the protected ref.
FROM node:22-slim
WORKDIR /app
COPY . .
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleEnvironment {
    pub scope: EnvironmentScope,
    pub reason: String,
    pub files: Vec<EnvironmentFile>,
}

/// What the setup agent would say for each scope.
pub fn sample_reason(scope: EnvironmentScope) -> &'static str {
    match scope {
        EnvironmentScope::None => {
            "This task is pure code and tests, so nothing needs to be deployed: the evaluator runs it in its container."
        }
        EnvironmentScope::Single => {
            "One machine measures this: a CLI / build / single service with nothing else to talk to. It runs on pinned hardware so every measurement is comparable."
        }
        EnvironmentScope::Full => {
            "This task needs the full stack: the API, Postgres with seeded orders, Redis, and a separate load generator. p95 depends on database queries, so a single machine wouldn't measure it."
        }
    }
}

/// Per-scope simulated drafting cost, shown as "2 min, $0.31": `(duration_s, usd_spent)`.
pub fn sample_effort(scope: EnvironmentScope) -> (u64, f64) {
    match scope {
        EnvironmentScope::None => (24, 0.04),
        EnvironmentScope::Single => (58, 0.12),
        EnvironmentScope::Full => (120, 0.31),
    }
}

#[derive(Debug, Clone, Default)]
pub struct SampleOptions {
    pub original_config_yaml: Option<String>,
    pub config_path: Option<String>,
    pub reason: Option<String>,
}

/// The example draft for `scope`: ratchet.yaml (the given one with `environment` set) plus the files under
/// `infra/ratchet/`. `none` writes only ratchet.yaml; `single` adds a Dockerfile and pins the machine; `full` is a
/// four-machine stack.
pub fn sample_environment(scope: EnvironmentScope, opts: &SampleOptions) -> SampleEnvironment {
    let original = opts
        .original_config_yaml
        .as_deref()
        .unwrap_or(MOCK_CONFIG_YAML);
    let config_path = opts.config_path.as_deref().unwrap_or("ratchet.yaml");
    let mut patch = EnvironmentConfigPatch::scope(scope);
    if scope == EnvironmentScope::Single {
        patch.machine = Some("c7i.xlarge".into());
        patch.image = Some(format!("{D}/Dockerfile"));
    }
    // the sample config is a fixed, valid document unless the caller hands in a broken one: keep it as it is then
    let config =
        apply_environment_to_config_yaml(original, &patch).unwrap_or_else(|_| original.to_owned());
    let mut files = vec![EnvironmentFile::new(config_path, config)];
    if scope == EnvironmentScope::Single {
        files.push(EnvironmentFile::new(
            format!("{D}/Dockerfile"),
            SINGLE_DOCKERFILE,
        ));
    }
    if scope == EnvironmentScope::Full {
        for (p, c) in [
            ("main.tf", FULL_MAIN_TF),
            ("Dockerfile", FULL_DOCKERFILE),
            ("seed.sh", FULL_SEED_SH),
            ("seed/orders.sql", FULL_ORDERS_SQL),
            ("scripts/api.sh", FULL_API_SH),
            ("scripts/redis.sh", FULL_REDIS_SH),
            ("scripts/load.sh", FULL_LOAD_SH),
        ] {
            files.push(EnvironmentFile::new(format!("{D}/{p}"), c));
        }
    }
    SampleEnvironment {
        scope,
        reason: opts
            .reason
            .clone()
            .unwrap_or_else(|| sample_reason(scope).to_owned()),
        files,
    }
}
