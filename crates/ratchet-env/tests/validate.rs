//! Port of `packages/core/src/environment/validate.test.ts`.

use ratchet_core::enums::EnvironmentScope;
use ratchet_env::{
    EnvironmentConfigPatch, EnvironmentFile, EnvironmentRole, EnvironmentValidation,
    ValidateEnvironmentInput, apply_environment_to_config_yaml, validate_environment,
};

const CONFIG: &str = "version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
evaluator:
  template: http-latency
";

const D: &str = "infra/ratchet";

const MAIN_TF: &str = r#"module "sandbox" {
  source  = "./ratchet-sandbox"
  ratchet = var.ratchet

  machines = {
    api = {
      role          = "app"
      label         = "API"
      detail        = "the service"
      instance_type = "c7i.2xlarge"
      user_data     = file("${path.module}/api.sh")
    }
    db = {
      role           = "database"
      label          = "Postgres"
      instance_type  = "r7g.large"
      data_volume_gb = 100
    }
  }
}
"#;

fn cfg(scope: EnvironmentScope) -> String {
    apply_environment_to_config_yaml(CONFIG, &EnvironmentConfigPatch::scope(scope)).unwrap()
}

#[derive(Default)]
struct Over {
    main: Option<String>,
    scope: Option<EnvironmentScope>,
    extra: Vec<EnvironmentFile>,
    config: Option<String>,
}

fn good(o: Over) -> ValidateEnvironmentInput {
    let scope = o.scope.unwrap_or(EnvironmentScope::Full);
    let mut files = vec![
        EnvironmentFile::new("ratchet.yaml", o.config.unwrap_or_else(|| cfg(scope))),
        EnvironmentFile::new(
            format!("{D}/main.tf"),
            o.main.unwrap_or_else(|| MAIN_TF.to_owned()),
        ),
        EnvironmentFile::new(
            format!("{D}/api.sh"),
            "#!/bin/bash\nset -e\ndocker run -d repo:ratchet\n",
        ),
    ];
    files.extend(o.extra);
    let mut i = ValidateEnvironmentInput::new(scope, files);
    i.original_config_yaml = Some(CONFIG.into());
    i
}

fn failing(r: &EnvironmentValidation) -> Vec<String> {
    r.checks
        .iter()
        .filter(|c| !c.ok)
        .map(|c| c.id.clone())
        .collect()
}
fn detail(r: &EnvironmentValidation, id: &str) -> String {
    r.checks.iter().find(|c| c.id == id).unwrap().detail.clone()
}
fn with_main(extra: &str) -> ValidateEnvironmentInput {
    good(Over {
        main: Some(format!("{MAIN_TF}\n{extra}")),
        ..Over::default()
    })
}
fn v(i: ValidateEnvironmentInput) -> EnvironmentValidation {
    validate_environment(&i)
}
fn main_with(main: String) -> ValidateEnvironmentInput {
    good(Over {
        main: Some(main),
        ..Over::default()
    })
}
fn extra_file(path: &str, content: &str) -> ValidateEnvironmentInput {
    good(Over {
        extra: vec![EnvironmentFile::new(path, content)],
        ..Over::default()
    })
}
fn ids(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| (*s).to_owned()).collect()
}

// ---- a good draft ----------------------------------------------------------------------------------------------

// "passes every static check and reports the machines, the rows and the cost"
#[test]
fn a_good_draft_passes_every_static_check() {
    let r = v(good(Over::default()));
    assert_eq!(failing(&r), Vec::<String>::new());
    assert!(r.ok);
    let order: Vec<&str> = r.checks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        order,
        [
            "isolated",
            "sealed",
            "same-hardware",
            "teardown",
            "files",
            "syntax",
            "config",
            "scope",
            "base-module",
            "local-access",
            "tagged",
            "cost"
        ]
    );
    let m: Vec<(Option<&str>, &str, &str, EnvironmentRole)> = r
        .machines
        .iter()
        .map(|m| {
            (
                m.key.as_deref(),
                m.name.as_str(),
                m.machine.as_str(),
                m.role,
            )
        })
        .collect();
    assert_eq!(
        m,
        vec![
            (Some("api"), "API", "c7i.2xlarge", EnvironmentRole::App),
            (
                Some("db"),
                "Postgres",
                "r7g.large",
                EnvironmentRole::Database
            )
        ]
    );
    assert_eq!(
        r.resources
            .iter()
            .map(|x| x.name.as_str())
            .collect::<Vec<_>>(),
        ["API", "Postgres"]
    );
    assert!(r.cost.usd_per_hour > 0.357 + 0.1071);
    assert_eq!(r.cost.startup_minutes, 4);
}

// "scope none needs only ratchet.yaml and is priced as the evaluator host"
#[test]
fn scope_none_needs_only_the_config() {
    let mut i = ValidateEnvironmentInput::new(
        EnvironmentScope::None,
        vec![EnvironmentFile::new(
            "ratchet.yaml",
            cfg(EnvironmentScope::None),
        )],
    );
    i.original_config_yaml = Some(CONFIG.into());
    let r = v(i);
    assert!(r.ok, "{:?}", r.problems);
    assert_eq!(
        r.machines.iter().map(|m| m.role).collect::<Vec<_>>(),
        [EnvironmentRole::Evaluator]
    );
}

// "scope single without OpenTofu pins the machine from ratchet.yaml and may carry a Dockerfile"
#[test]
fn scope_single_without_opentofu_pins_the_machine() {
    let mut p = EnvironmentConfigPatch::scope(EnvironmentScope::Single);
    p.machine = Some("c7i.xlarge".into());
    p.image = Some(format!("{D}/Dockerfile"));
    let c = apply_environment_to_config_yaml(CONFIG, &p).unwrap();
    let mut i = ValidateEnvironmentInput::new(
        EnvironmentScope::Single,
        vec![
            EnvironmentFile::new("ratchet.yaml", c),
            EnvironmentFile::new(format!("{D}/Dockerfile"), "FROM node:22\n"),
        ],
    );
    i.original_config_yaml = Some(CONFIG.into());
    let r = v(i);
    assert!(r.ok, "{:?}", r.problems);
    assert_eq!(
        (r.machines[0].machine.as_str(), r.machines[0].role),
        ("c7i.xlarge", EnvironmentRole::App)
    );
}

// "a heredoc user_data and inline machine objects are fine"
#[test]
fn heredoc_user_data_and_inline_machine_objects_are_fine() {
    let main = "module \"sandbox\" {\n  source  = \"./ratchet-sandbox\"\n  ratchet = var.ratchet\n  machines = {\n    a = { role = \"app\", instance_type = \"c7i.large\", user_data = <<-EOT\n      #!/bin/bash\n      echo hi\n    EOT\n    }\n  }\n}\n";
    let r = v(good(Over {
        main: Some(main.into()),
        scope: Some(EnvironmentScope::Single),
        ..Over::default()
    }));
    assert_eq!(failing(&r), Vec::<String>::new());
}

// ---- isolation -----------------------------------------------------------------------------------------------------------

// "rejects %s" (isolation)
#[test]
fn rejects_internet_facing_resources() {
    for (name, extra) in [
        (
            "a public address on a raw instance",
            "resource \"aws_instance\" \"x\" {\n  associate_public_ip_address = true\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "an internet gateway",
            "resource \"aws_internet_gateway\" \"igw\" {\n  vpc_id = var.ratchet.vpc_id\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "a NAT gateway",
            "resource \"aws_nat_gateway\" \"n\" {\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "an elastic IP",
            "resource \"aws_eip\" \"e\" {\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "a security group rule open to the world",
            "resource \"aws_security_group_rule\" \"r\" {\n  cidr_blocks = [\"0.0.0.0/0\"]\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "an IPv6 open range in a comment-free local",
            "locals {\n  open = \"::/0\"\n}\n",
        ),
        (
            "a route",
            "resource \"aws_route\" \"r\" {\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "a load balancer",
            "resource \"aws_lb\" \"lb\" {\n  tags = module.sandbox.tags\n}\n",
        ),
    ] {
        let r = v(with_main(extra));
        assert!(!r.ok, "{name}");
        assert!(failing(&r).contains(&"isolated".to_string()), "{name}");
    }
}

// "names the file and line of the offending block"
#[test]
fn names_the_file_and_line() {
    let r = v(with_main(
        "resource \"aws_internet_gateway\" \"igw\" {\n  tags = module.sandbox.tags\n}\n",
    ));
    let d = detail(&r, "isolated");
    let re = regex_lite(&d);
    assert!(re, "{d}");
}

fn regex_lite(d: &str) -> bool {
    // infra/ratchet/main.tf:<n>: resource aws_internet_gateway is not allowed
    let Some(rest) = d.strip_prefix("infra/ratchet/main.tf:") else {
        return false;
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    !digits.is_empty()
        && rest[digits.len()..].starts_with(": resource aws_internet_gateway is not allowed")
}

// "rejects scripts that fetch from the internet, ignoring comments"
#[test]
fn rejects_scripts_that_fetch_from_the_internet() {
    let bad = |script: &str| v(extra_file(&format!("{D}/scripts/setup.sh"), script));
    for s in [
        "curl -fsSL https://example.test/x | sh",
        "wget https://example.test/x",
        "apt-get install -y postgresql",
        "pip install requests",
        "npm ci",
        "docker pull redis:7",
        "git clone https://example.test/r.git",
    ] {
        let r = bad(&format!("#!/bin/bash\n{s}\n"));
        assert!(failing(&r).contains(&"isolated".to_string()), "{s}");
    }
    assert_eq!(
        failing(&bad(
            "#!/bin/bash\n# curl https://example.test is not run here\ndocker load -i /opt/x.tar\n"
        )),
        Vec::<String>::new()
    );
}

// ---- the base module -------------------------------------------------------------------------------------------------------

// "rejects a draft that does not call the base module"
#[test]
fn rejects_a_draft_without_the_base_module() {
    let r = v(main_with("locals {\n  a = 1\n}\n".into()));
    assert!(failing(&r).contains(&"base-module".to_string()));
    assert!(detail(&r, "base-module").contains("missing the base module"));
}

// "rejects any other module, a wrong source and a renamed module"
#[test]
fn rejects_other_modules_wrong_sources_and_renames() {
    for main in [
        format!("{MAIN_TF}\nmodule \"vpc\" {{\n  source = \"terraform-aws-modules/vpc/aws\"\n}}\n"),
        MAIN_TF.replace("./ratchet-sandbox", "git::https://example.test/evil.git"),
        MAIN_TF.replace("module \"sandbox\"", "module \"mine\""),
    ] {
        assert!(
            failing(&v(main_with(main.clone()))).contains(&"base-module".to_string()),
            "{main}"
        );
    }
}

// "requires the sandbox to be passed through, and forbids count / for_each / providers on the call"
#[test]
fn requires_the_sandbox_pass_through() {
    assert!(
        failing(&v(main_with(
            MAIN_TF.replace("ratchet = var.ratchet", "ratchet = {}")
        )))
        .contains(&"base-module".to_string())
    );
    let r = v(main_with(
        MAIN_TF.replace("source  =", "count = 2\n  source  ="),
    ));
    assert!(detail(&r, "base-module").contains("may not set count"));
}

// "forbids providers, backends, required_providers, the generated variable and the generated output"
#[test]
fn forbids_providers_backends_and_generated_names() {
    for extra in [
        "provider \"aws\" {\n  region = \"eu-west-1\"\n}\n",
        "terraform {\n  backend \"s3\" {}\n}\n",
        "terraform {\n  required_providers {\n    aws = { source = \"evil/aws\" }\n  }\n}\n",
        "variable \"ratchet\" {\n  type = any\n}\n",
        "output \"environment\" {\n  value = 1\n}\n",
    ] {
        assert!(!v(with_main(extra)).ok, "{extra}");
    }
}

// "forbids machine keys that place or identify a machine, and unknown roles"
#[test]
fn forbids_placement_keys_and_unknown_roles() {
    let r = v(main_with(MAIN_TF.replace(
        "role          = \"app\"",
        "role = \"app\"\n      subnet_id = \"subnet-123\"\n      iam_instance_profile = \"admin\"",
    )));
    assert!(failing(&r).contains(&"sealed".to_string()));
    assert!(detail(&r, "sealed").contains("sets \"subnet_id\""));
    assert!(
        failing(&v(main_with(MAIN_TF.replace("\"app\"", "\"admin\""))))
            .contains(&"base-module".to_string())
    );
}

// "limits the number of machines"
#[test]
fn limits_the_number_of_machines() {
    let many: Vec<String> = (0..9)
        .map(|i| {
            format!(
                "    m{i} = {{ role = \"other\", instance_type = \"c7i.large\", label = \"M{i}\" }}"
            )
        })
        .collect();
    let r = v(main_with(format!(
        "module \"sandbox\" {{\n  source = \"./ratchet-sandbox\"\n  ratchet = var.ratchet\n  machines = {{\n{}\n  }}\n}}\n",
        many.join("\n")
    )));
    assert!(detail(&r, "base-module").contains("9 machines"));
}

// ---- sealed -----------------------------------------------------------------------------------------------------------------

// "rejects %s" (sealed)
#[test]
fn rejects_identity_and_sealed_store_access() {
    for (name, extra) in [
        (
            "an IAM role",
            "resource \"aws_iam_role\" \"r\" {\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "an IAM policy attachment",
            "resource \"aws_iam_role_policy_attachment\" \"a\" {\n}\n",
        ),
        (
            "an instance profile",
            "resource \"aws_instance\" \"x\" {\n  iam_instance_profile = \"p\"\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "a reference to the sealed store",
            "locals {\n  b = \"ratchet-demo-sealed\"\n}\n",
        ),
        (
            "a reference to the results prefix",
            "locals {\n  b = \"bench-results/\"\n}\n",
        ),
        (
            "an s3 object outside env/",
            "resource \"aws_s3_object\" \"o\" {\n  bucket = module.sandbox.artifacts_bucket\n  key = \"bench-inputs/x\"\n  content = \"x\"\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "an s3 object in another bucket",
            "resource \"aws_s3_object\" \"o\" {\n  bucket = \"somebody-elses-bucket\"\n  key = \"env/x\"\n  content = \"x\"\n  tags = module.sandbox.tags\n}\n",
        ),
    ] {
        let r = v(with_main(extra));
        assert!(!r.ok, "{name}");
        assert!(
            failing(&r)
                .iter()
                .any(|id| id == "sealed" || id == "isolated"),
            "{name}"
        );
    }
}

// "accepts seed data under env/ in the artifacts bucket, tagged"
#[test]
fn accepts_seed_data_under_env() {
    let extra = "resource \"aws_s3_object\" \"seed\" {\n  bucket  = module.sandbox.artifacts_bucket\n  key     = \"env/seed/orders.sql\"\n  content = file(\"${path.module}/seed.sql\")\n  tags    = module.sandbox.tags\n}\n";
    let mut i = with_main(extra);
    i.files
        .push(EnvironmentFile::new(format!("{D}/seed.sql"), "select 1;"));
    assert_eq!(failing(&v(i)), Vec::<String>::new());
}

// ---- tags -------------------------------------------------------------------------------------------------------------------

// "rejects a resource without the sandbox tags"
#[test]
fn rejects_a_resource_without_the_sandbox_tags() {
    let extra = "resource \"aws_s3_object\" \"seed\" {\n  bucket  = module.sandbox.artifacts_bucket\n  key     = \"env/seed.sql\"\n  content = \"x\"\n}\n";
    let r = v(with_main(extra));
    assert_eq!(failing(&r), ids(&["tagged"]));
    assert!(
        detail(&r, "tagged").contains("aws_s3_object.seed must set tags = module.sandbox.tags")
    );
}

// "accepts tags merged with the sandbox tags"
#[test]
fn accepts_merged_tags() {
    let extra = "resource \"aws_s3_object\" \"seed\" {\n  bucket  = var.ratchet.artifacts_bucket\n  key     = \"env/seed.sql\"\n  content = \"x\"\n  tags    = merge(module.sandbox.tags, { a = \"b\" })\n}\n";
    assert_eq!(failing(&v(with_main(extra))), Vec::<String>::new());
}

// ---- same hardware --------------------------------------------------------------------------------------------------------

// "requires literal instance types and a literal machines map"
#[test]
fn requires_literal_instance_types_and_machines() {
    for repl in ["var.size", "\"${var.family}.2xlarge\""] {
        assert!(
            failing(&v(main_with(MAIN_TF.replace("\"c7i.2xlarge\"", repl))))
                .contains(&"same-hardware".to_string()),
            "{repl}"
        );
    }
    let r = v(good(Over {
        main: Some("module \"sandbox\" {\n  source = \"./ratchet-sandbox\"\n  ratchet = var.ratchet\n  machines = var.machines\n}\n".into()),
        scope: Some(EnvironmentScope::Single),
        ..Over::default()
    }));
    assert!(detail(&r, "same-hardware").contains("literal map"));
}

// "rejects burstable types for the machines the metric is measured on, allows them for a cache"
#[test]
fn rejects_burstable_types_except_for_a_cache() {
    let r = v(main_with(
        MAIN_TF.replace("\"c7i.2xlarge\"", "\"t3.large\""),
    ));
    assert_eq!(failing(&r), ids(&["same-hardware"]));
    assert!(detail(&r, "same-hardware").contains("burstable"));
    let cache = MAIN_TF
        .replace("\"r7g.large\"", "\"t4g.medium\"")
        .replace("\"database\"", "\"cache\"");
    assert_eq!(failing(&v(main_with(cache))), Vec::<String>::new());
}

// ---- teardown ---------------------------------------------------------------------------------------------------------------

// "rejects %s" (teardown)
#[test]
fn rejects_what_blocks_teardown() {
    for (name, extra) in [
        (
            "prevent_destroy",
            "resource \"aws_s3_object\" \"o\" {\n  bucket = var.ratchet.artifacts_bucket\n  key = \"env/x\"\n  content = \"x\"\n  tags = module.sandbox.tags\n  lifecycle {\n    prevent_destroy = true\n  }\n}\n",
        ),
        (
            "deletion protection",
            "locals {\n}\nresource \"aws_s3_object\" \"o\" {\n  deletion_protection = true\n  bucket = var.ratchet.artifacts_bucket\n  key = \"env/x\"\n  content = \"x\"\n  tags = module.sandbox.tags\n}\n",
        ),
        (
            "an import block",
            "import {\n  to = aws_s3_object.o\n  id = \"x\"\n}\n",
        ),
        (
            "force_destroy = false",
            "resource \"aws_s3_object\" \"o\" {\n  force_destroy = false\n  bucket = var.ratchet.artifacts_bucket\n  key = \"env/x\"\n  content = \"x\"\n  tags = module.sandbox.tags\n}\n",
        ),
    ] {
        assert!(
            failing(&v(with_main(extra))).contains(&"teardown".to_string()),
            "{name}"
        );
    }
}

// ---- nothing runs or is read on the user machine -----------------------------------------------------------------------

// "rejects %s" (local access)
#[test]
fn rejects_what_runs_or_reads_locally() {
    for (name, extra) in [
        (
            "a local-exec provisioner",
            "resource \"aws_s3_object\" \"o\" {\n  bucket = var.ratchet.artifacts_bucket\n  key = \"env/x\"\n  content = \"x\"\n  tags = module.sandbox.tags\n  provisioner \"local-exec\" {\n    command = \"curl x\"\n  }\n}\n",
        ),
        (
            "an external data source",
            "data \"external\" \"e\" {\n  program = [\"sh\", \"-c\", \"id\"]\n}\n",
        ),
        ("path.cwd", "locals {\n  p = path.cwd\n}\n"),
        (
            "a data source outside the allow-list",
            "data \"aws_ami\" \"a\" {\n  most_recent = true\n}\n",
        ),
    ] {
        assert!(
            failing(&v(with_main(extra))).contains(&"local-access".to_string()),
            "{name}"
        );
    }
}

// "rejects file() outside the draft and file() of a file that is not in it"
#[test]
fn rejects_file_outside_the_draft() {
    let call = "file(\"${path.module}/api.sh\")";
    let abs = v(main_with(
        MAIN_TF.replace(call, "file(\"/home/user/.aws/credentials\")"),
    ));
    assert!(failing(&abs).contains(&"local-access".to_string()));
    let up = v(main_with(
        MAIN_TF.replace(call, "file(\"${path.module}/../../.env\")"),
    ));
    assert!(failing(&up).contains(&"local-access".to_string()));
    let missing = v(main_with(MAIN_TF.replace("api.sh", "nope.sh")));
    assert!(detail(&missing, "local-access").contains("nope.sh, which is not part of the draft"));
    let home = v(main_with(
        MAIN_TF.replace(call, "file(pathexpand(\"~/.ssh/id_rsa\"))"),
    ));
    assert!(failing(&home).contains(&"local-access".to_string()));
}

// "rejects an s3 object that uploads a local file by absolute path"
#[test]
fn rejects_s3_upload_of_an_absolute_path() {
    let extra = "resource \"aws_s3_object\" \"o\" {\n  bucket = var.ratchet.artifacts_bucket\n  key = \"env/x\"\n  source = \"/etc/passwd\"\n  tags = module.sandbox.tags\n}\n";
    assert!(failing(&v(with_main(extra))).contains(&"local-access".to_string()));
}

// ---- files ---------------------------------------------------------------------------------------------------------------------

// "only ratchet.yaml and infra/ratchet/ may be part of a draft"
#[test]
fn only_the_config_and_the_infra_dir_may_be_part_of_a_draft() {
    let r = v(good(Over {
        extra: vec![
            EnvironmentFile::new("src/server.ts", "x"),
            EnvironmentFile::new("bench/run.mjs", "x"),
        ],
        ..Over::default()
    }));
    assert_eq!(failing(&r), ids(&["files"]));
    assert!(detail(&r, "files").contains("src/server.ts: a drafted environment may only change ratchet.yaml and files under infra/ratchet/"));
}

// "rejects %s" (files)
#[test]
fn rejects_unsafe_and_forbidden_files() {
    for (name, path) in [
        ("a path that leaves the repo", "../x".to_string()),
        ("an absolute path", "/etc/x".to_string()),
        ("a .git path", format!("{D}/.git/config")),
        ("a tfvars file", format!("{D}/prod.tfvars")),
        ("an override file", format!("{D}/main_override.tf")),
        ("JSON configuration", format!("{D}/extra.tf.json")),
        ("a generated file", format!("{D}/ratchet_inputs.tf")),
        (
            "a copy of the base module",
            format!("{D}/ratchet-sandbox/main.tf"),
        ),
        ("state", format!("{D}/terraform.tfstate")),
    ] {
        assert!(
            failing(&v(extra_file(&path, "x"))).contains(&"files".to_string()),
            "{name}"
        );
    }
}

// "limits file and total size"
#[test]
fn limits_file_and_total_size() {
    let big = "x".repeat(300 * 1024);
    assert!(failing(&v(extra_file(&format!("{D}/big.sql"), &big))).contains(&"files".to_string()));
}

// "refuses a duplicate path"
#[test]
fn refuses_a_duplicate_path() {
    let mut g = good(Over::default());
    let dup = g.files[1].clone();
    g.files.push(dup);
    assert!(failing(&v(g)).contains(&"files".to_string()));
}

// ---- syntax / ratchet.yaml ---------------------------------------------------------------------------------------------

// "reports unreadable OpenTofu"
#[test]
fn reports_unreadable_opentofu() {
    let r = v(main_with(format!(
        "{MAIN_TF}\nresource \"aws_s3_object\" \"x\" {{\n  a = 1\n"
    )));
    assert!(failing(&r).contains(&"syntax".to_string()));
}

// "requires the config and a matching environment.scope"
#[test]
fn requires_the_config_and_a_matching_scope() {
    let mut g = good(Over::default());
    g.files.retain(|f| f.path != "ratchet.yaml");
    assert!(failing(&v(g)).contains(&"config".to_string()));
    let r = v(good(Over {
        config: Some(CONFIG.into()),
        ..Over::default()
    }));
    assert!(detail(&r, "config").contains("declares environment.scope (none), the draft is full"));
    let r = v(good(Over {
        config: Some(cfg(EnvironmentScope::Single)),
        ..Over::default()
    }));
    assert!(detail(&r, "config").contains("declares environment.scope single"));
}

// "a draft may change only environment: not the objective, the evaluator, the guardrails or the protected paths"
#[test]
fn a_draft_may_change_only_environment() {
    let tampered = cfg(EnvironmentScope::Full).replace("minimize", "maximize");
    let r = v(good(Over {
        config: Some(tampered),
        ..Over::default()
    }));
    assert_eq!(failing(&r), ids(&["config"]));
    assert!(detail(&r, "config").contains("changes objective"));
    let guard = format!(
        "{}protected:\n  - nothing/**\n",
        cfg(EnvironmentScope::Full)
    );
    let r = v(good(Over {
        config: Some(guard),
        ..Over::default()
    }));
    assert!(detail(&r, "config").contains("changes protected"));
}

// "a draft may not grant itself egress hosts"
#[test]
fn a_draft_may_not_grant_itself_egress_hosts() {
    let c = cfg(EnvironmentScope::Full)
        .replace("scope: full", "scope: full\n  network: [evil.example.com]");
    let r = v(good(Over {
        config: Some(c),
        ..Over::default()
    }));
    assert!(detail(&r, "config").contains("environment.network adds evil.example.com"));
}

// "rejects a ratchet.yaml that is not valid"
#[test]
fn rejects_an_invalid_config() {
    assert!(
        failing(&v(good(Over {
            config: Some("version: 7\n".into()),
            ..Over::default()
        })))
        .contains(&"config".to_string())
    );
}

// ---- scope and cost -----------------------------------------------------------------------------------------------------

// "scope none must not ship infrastructure"
#[test]
fn scope_none_must_not_ship_infrastructure() {
    let r = v(good(Over {
        scope: Some(EnvironmentScope::None),
        ..Over::default()
    }));
    assert!(detail(&r, "scope").contains("scope none needs no infrastructure"));
}

// "scope single is exactly one machine; scope full is a stack of at least two"
#[test]
fn scope_single_is_one_machine_and_full_at_least_two() {
    let r = v(good(Over {
        scope: Some(EnvironmentScope::Single),
        ..Over::default()
    }));
    assert!(detail(&r, "scope").contains("one machine, the draft has 2"));
    let one = "module \"sandbox\" {\n  source = \"./ratchet-sandbox\"\n  ratchet = var.ratchet\n  machines = {\n    a = { role = \"app\", instance_type = \"c7i.large\" }\n  }\n}\n";
    let r = v(good(Over {
        main: Some(one.into()),
        scope: Some(EnvironmentScope::Full),
        ..Over::default()
    }));
    assert!(detail(&r, "scope").contains("at least two machines"));
    let r = v(good(Over {
        main: Some(one.into()),
        scope: Some(EnvironmentScope::Single),
        ..Over::default()
    }));
    assert_eq!(failing(&r), Vec::<String>::new());
    let mut i = ValidateEnvironmentInput::new(
        EnvironmentScope::Full,
        vec![EnvironmentFile::new(
            "ratchet.yaml",
            cfg(EnvironmentScope::Full),
        )],
    );
    i.original_config_yaml = Some(CONFIG.into());
    assert!(detail(&v(i), "scope").contains("needs OpenTofu"));
}

// "rejects an instance type with no known price (the cost has to be shown before anything is created)"
#[test]
fn rejects_an_unpriced_instance_type() {
    let r = v(main_with(
        MAIN_TF.replace("\"c7i.2xlarge\"", "\"x2gd.metal\""),
    ));
    assert_eq!(failing(&r), ids(&["cost"]));
    assert!(detail(&r, "cost").contains("no price known for x2gd.metal"));
}

// "module-only mode (what the engine and the provider re-check on apply) skips the config check"
#[test]
fn module_only_mode_skips_the_config_check() {
    let g = good(Over::default());
    let mut i = ValidateEnvironmentInput::new(
        EnvironmentScope::Full,
        g.files
            .iter()
            .filter(|f| f.path != "ratchet.yaml")
            .cloned()
            .collect(),
    );
    i.module_only = true;
    assert_eq!(failing(&v(i)), Vec::<String>::new());
    let mut bad = ValidateEnvironmentInput::new(
        EnvironmentScope::Full,
        vec![EnvironmentFile::new(
            format!("{D}/main.tf"),
            format!("{MAIN_TF}\nresource \"aws_internet_gateway\" \"i\" {{\n}}\n"),
        )],
    );
    bad.module_only = true;
    assert!(failing(&v(bad)).contains(&"isolated".to_string()));
}
