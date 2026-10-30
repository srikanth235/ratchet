import { describe, expect, it } from "vitest";
import { readAws } from "./render.js";

const dns = readAws("dns_firewall.tf");
const vars = readAws("variables.tf");
const s3 = readAws("s3.tf");

const resource = (src: string, type: string, name: string) => {
  const i = src.indexOf(`resource "${type}" "${name}"`);
  expect(i, `${type}.${name} is declared`).toBeGreaterThan(-1);
  return src.slice(i).split(/\nresource /)[0]!;
};

describe("aws module: DNS firewall", () => {
  it("is on by default and every resource is conditional on the switch", () => {
    expect(vars).toMatch(/variable "enable_dns_firewall"[\s\S]*?default\s*=\s*true/);
    for (const [t, n] of [
      ["aws_route53_resolver_firewall_domain_list", "allowed"],
      ["aws_route53_resolver_firewall_domain_list", "everything"],
      ["aws_route53_resolver_firewall_rule_group", "sandbox"],
      ["aws_route53_resolver_firewall_rule", "allow_listed"],
      ["aws_route53_resolver_firewall_rule", "block_everything_else"],
      ["aws_route53_resolver_firewall_rule_group_association", "sandbox"],
      ["aws_route53_resolver_firewall_config", "sandbox"],
    ] as const) {
      expect(resource(dns, t, n)).toContain("count = var.enable_dns_firewall ? 1 : 0");
    }
  });

  it("associates the group with the sandbox VPC and fails closed", () => {
    expect(resource(dns, "aws_route53_resolver_firewall_rule_group_association", "sandbox")).toContain("vpc_id                 = aws_vpc.sandbox.id");
    expect(resource(dns, "aws_route53_resolver_firewall_config", "sandbox")).toMatch(/firewall_fail_open\s*=\s*"DISABLED"/);
  });

  it("allows first, blocks the rest with NXDOMAIN", () => {
    const allow = resource(dns, "aws_route53_resolver_firewall_rule", "allow_listed");
    const block = resource(dns, "aws_route53_resolver_firewall_rule", "block_everything_else");
    expect(allow).toMatch(/action\s*=\s*"ALLOW"/);
    expect(block).toMatch(/action\s*=\s*"BLOCK"/);
    expect(block).toMatch(/block_response\s*=\s*"NXDOMAIN"/);
    const prio = (b: string) => Number(/priority\s*=\s*(\d+)/.exec(b)![1]);
    expect(prio(allow)).toBeLessThan(prio(block));
    expect(resource(dns, "aws_route53_resolver_firewall_domain_list", "everything")).toMatch(/domains\s*=\s*\["\*"\]/);
  });

  it("derives the bench names from the interface endpoint list, so they cannot drift apart", () => {
    expect(dns).toContain("for s in local.interface_endpoint_services");
    expect(dns).toContain('"${s}.${var.region}.amazonaws.com"');
    expect(dns).toContain("*.s3.${var.region}.amazonaws.com");
  });

  it("builds the agent names from the proxy allow-list instead of repeating them", () => {
    expect(dns).toContain("local.proxy_allowed_domains");
    expect(dns).toContain('trimprefix(d, ".")');
  });

  it("never allows a catch-all or a whole public suffix, in the module or through the extras variable", () => {
    expect(dns).not.toMatch(/"\*\.amazonaws\.com"|"\*\.com"|"\*\.org"/);
    const check = /variable "dns_firewall_extra_domains"[\s\S]*?condition\s*=\s*(.*)/.exec(vars)![1]!;
    expect(check).toContain("[a-z]{2,}"); // needs a TLD, so a bare "*" fails
    expect(check).toContain("alltrue(");
  });

  it("the bench security group still has no internet egress (DNS firewall is an extra layer, not a replacement)", () => {
    const net = readAws("network.tf");
    const bench = net.slice(net.indexOf('resource "aws_security_group" "bench"')).split(/\nresource /)[0]!;
    expect(bench).not.toContain("0.0.0.0/0");
  });
});

describe("aws module: bench-inputs lifecycle", () => {
  const lifecycle = resource(s3, "aws_s3_bucket_lifecycle_configuration", "artifacts");
  const rule = lifecycle.slice(lifecycle.indexOf('id     = "expire-bench-inputs"'));

  it("has its own rule on the bench-inputs/ prefix, driven by a variable that defaults to 7 days", () => {
    expect(rule).toMatch(/prefix\s*=\s*"bench-inputs\/"/);
    expect(rule).toMatch(/expiration\s*\{\s*days\s*=\s*var\.bench_inputs_retention_days/);
    expect(vars).toMatch(/variable "bench_inputs_retention_days"[\s\S]*?default\s*=\s*7/);
  });

  it("also expires noncurrent versions and aborts incomplete multipart uploads", () => {
    expect(rule).toMatch(/noncurrent_version_expiration\s*\{\s*noncurrent_days\s*=\s*var\.bench_inputs_retention_days/);
    expect(rule).toContain("abort_incomplete_multipart_upload");
  });

  it("keeps the general rule for the rest of the bucket", () => {
    expect(lifecycle).toContain('id     = "expire-artifacts"');
    expect(lifecycle).toContain("var.artifacts_retention_days");
  });

  it("the bench runner can still read bench-inputs/ (the lifecycle must not outrun a job)", () => {
    expect(readAws("policies/bench_runner_policy.json.tftpl")).toContain("bench-inputs/*");
  });
});
