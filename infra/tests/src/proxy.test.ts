import { describe, expect, it } from "vitest";
import { readAws, renderTemplate } from "./render.js";

const tf = (n: string) => readAws(n);
const conf = renderTemplate(readAws("templates/squid.conf.tftpl"), {
  proxy_port: "3128",
  agent_subnet_cidr: "10.42.2.0/24",
  allowed_domains: "api.anthropic.com api.openai.com .github.com .githubusercontent.com ec2.us-east-1.amazonaws.com",
});
const lines = conf.split("\n").filter((l) => l.trim() && !l.startsWith("#"));

describe("egress proxy: squid config", () => {
  it("only serves the agent subnet and ends in deny all", () => {
    expect(lines).toContain("acl agent_subnet src 10.42.2.0/24");
    expect(lines).toContain("http_access deny !agent_subnet");
    const access = lines.filter((l) => l.startsWith("http_access"));
    expect(access[access.length - 1]).toBe("http_access deny all");
  });
  it("allows nothing but CONNECT to port 443 on allow-listed domains", () => {
    const access = lines.filter((l) => l.startsWith("http_access"));
    expect(access.filter((l) => l.startsWith("http_access allow"))).toEqual(["http_access allow connect_method allowed_domains"]);
    expect(access).toContain("http_access deny !connect_method"); // no plain HTTP forwarding
    expect(access).toContain("http_access deny connect_method !tls_port");
    expect(lines).toContain("acl tls_port port 443");
  });
  it("carries the model APIs, GitHub and AWS endpoints, and no catch-all domain", () => {
    const domains = lines.find((l) => l.startsWith("acl allowed_domains dstdomain"))!.split(" ").slice(3);
    for (const d of ["api.anthropic.com", "api.openai.com", ".github.com", ".githubusercontent.com"]) expect(domains).toContain(d);
    expect(domains.filter((d) => /^\.?(amazonaws\.com|com|\*)$/.test(d))).toEqual([]);
  });
  it("does not cache or leak the client address", () => {
    expect(lines).toContain("cache deny all");
    expect(lines).toContain("forwarded_for delete");
  });
});

describe("egress proxy: module wiring", () => {
  const proxy = tf("proxy.tf");
  it("allow-lists AWS service endpoints by exact regional hostname, not *.amazonaws.com", () => {
    expect(proxy).toContain('"ec2.${var.region}.amazonaws.com"');
    expect(proxy).toContain('"${aws_s3_bucket.artifacts.bucket}.s3.${var.region}.amazonaws.com"');
    expect(proxy).not.toMatch(/"\.?amazonaws\.com"/);
  });
  it("has no instance profile, requires IMDSv2 and only takes proxy traffic from the agent subnet", () => {
    expect(proxy).not.toContain("iam_instance_profile");
    expect(proxy).toMatch(/http_tokens\s*=\s*"required"/);
    const sg = proxy.slice(proxy.indexOf('resource "aws_security_group" "egress_proxy"')).split(/\nresource /)[0]!;
    const ingress = [...sg.matchAll(/\bingress\s*\{([^}]*)\}/g)].map((m) => m[1]!);
    expect(ingress).toHaveLength(1);
    expect(ingress[0]).toContain("local.agent_subnet_cidr");
    expect(ingress[0]).not.toContain("0.0.0.0/0");
  });
  it("runner egress in proxy mode names the proxy, the endpoint group and the S3 prefix list only", () => {
    const net = tf("network.tf");
    const sg = net.slice(net.indexOf('resource "aws_security_group" "agent_runner"')).split(/\nresource /)[0]!;
    const proxyDynamic = sg.split("dynamic").filter((p) => p.includes("var.enable_egress_proxy ? [1] : []"));
    expect(proxyDynamic).toHaveLength(3);
    expect(proxyDynamic.join("")).not.toContain("0.0.0.0/0");
  });
});
