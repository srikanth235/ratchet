import { readFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import type http from "node:http";
import { parse } from "yaml";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createServer } from "../../src/server.js";

type Schema = {
  $ref?: string; type?: string; enum?: unknown[]; required?: string[]; properties?: Record<string, Schema>;
  additionalProperties?: boolean; items?: Schema; minimum?: number; maximum?: number; maxLength?: number;
};
const spec = parse(readFileSync(new URL("../../openapi.yaml", import.meta.url), "utf8"));

/** Minimal OpenAPI-schema validator (enough for the subset used in openapi.yaml). */
function validate(value: unknown, schema: Schema, path = "$"): string[] {
  if (schema.$ref) return validate(value, spec.components.schemas[schema.$ref.split("/").pop()!], path);
  const errors: string[] = [];
  const t = schema.type;
  if (t === "integer" ? !Number.isInteger(value) : t === "array" ? !Array.isArray(value) : t === "object" ? typeof value !== "object" || value === null || Array.isArray(value) : typeof value !== t) {
    return [`${path}: expected ${t}`];
  }
  if (schema.enum && !schema.enum.includes(value)) errors.push(`${path}: not in enum`);
  if (typeof value === "number") {
    if (schema.minimum !== undefined && value < schema.minimum) errors.push(`${path}: below minimum`);
    if (schema.maximum !== undefined && value > schema.maximum) errors.push(`${path}: above maximum`);
  }
  if (t === "array") (value as unknown[]).forEach((v, i) => errors.push(...validate(v, schema.items!, `${path}[${i}]`)));
  if (t === "object") {
    const obj = value as Record<string, unknown>;
    for (const key of schema.required ?? []) if (!(key in obj)) errors.push(`${path}.${key}: missing`);
    for (const [key, v] of Object.entries(obj)) {
      const sub = schema.properties?.[key];
      if (sub) errors.push(...validate(v, sub, `${path}.${key}`));
      else if (schema.additionalProperties === false) errors.push(`${path}.${key}: unexpected property`);
    }
  }
  return errors;
}

let server: http.Server;
let base: string;
beforeAll(async () => {
  server = createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => new Promise<void>((resolve) => server.close(() => resolve())));

const responseSchema = (path: string, status: string): Schema =>
  spec.paths[path].get.responses[status].content["application/json"].schema;
const get = async (path: string) => {
  const res = await fetch(base + path);
  return { status: res.status, type: res.headers.get("content-type"), body: await res.json() };
};

describe("GET /health", () => {
  it("matches the spec", async () => {
    const r = await get("/health");
    expect(r.status).toBe(200);
    expect(r.type).toContain("application/json");
    expect(validate(r.body, responseSchema("/health", "200"))).toEqual([]);
    expect(r.body.products).toBe(50_000);
  });
});

describe("GET /search shapes", () => {
  it("matches the spec and echoes paging defaults", async () => {
    const r = await get("/search?q=kettle");
    expect(r.status).toBe(200);
    expect(validate(r.body, responseSchema("/search", "200"))).toEqual([]);
    expect(r.body.limit).toBe(20);
    expect(r.body.offset).toBe(0);
    expect(r.body.results.length).toBeLessThanOrEqual(20);
  });

  it("returns an empty page for no match", async () => {
    const r = await get("/search?q=zzzznomatch");
    expect(r.body).toEqual({ total: 0, limit: 20, offset: 0, results: [] });
  });

  it("browses everything when q and filters are absent", async () => {
    const r = await get("/search?limit=5");
    expect(r.body.total).toBe(50_000);
    expect(r.body.results).toHaveLength(5);
  });

  it("honours limit and offset", async () => {
    const a = await get("/search?q=compact&limit=100");
    const b = await get("/search?q=compact&limit=10&offset=10");
    expect(b.body.results).toEqual(a.body.results.slice(10, 20));
    expect(b.body.total).toBe(a.body.total);
  });
});

describe("GET /search errors", () => {
  it.each([
    ["/search?limit=0", "invalid_request"],
    ["/search?limit=101", "invalid_request"],
    ["/search?limit=abc", "invalid_request"],
    ["/search?offset=-1", "invalid_request"],
    ["/search?filters=price:%3Cabc", "invalid_filters"],
    ["/search?filters=category%3Akitchen%20AND", "invalid_filters"],
    [`/search?q=${"a".repeat(201)}`, "invalid_request"],
  ])("%s -> 400 %s", async (path, code) => {
    const r = await get(path);
    expect(r.status).toBe(400);
    expect(validate(r.body, responseSchema("/search", "400"))).toEqual([]);
    expect(r.body.error.code).toBe(code);
  });

  it("404s unknown routes and 405s other methods", async () => {
    expect((await get("/nope")).status).toBe(404);
    const res = await fetch(base + "/search", { method: "POST" });
    expect(res.status).toBe(405);
  });
});

interface Golden {
  query: { q?: string; filters?: string; limit?: number; offset?: number };
  total: number;
  ids: number[];
  scores: number[];
}
const golden = JSON.parse(readFileSync(new URL("./golden.json", import.meta.url), "utf8")) as Golden[];

describe("result ordering is pinned", () => {
  it.each(golden.map((g) => [JSON.stringify(g.query), g] as const))("%s", async (_name, g) => {
    const sp = new URLSearchParams();
    for (const [k, v] of Object.entries(g.query)) sp.set(k, String(v));
    const r = await get(`/search?${sp}`);
    expect(r.status).toBe(200);
    expect(r.body.total).toBe(g.total);
    expect(r.body.results.map((h: { id: number }) => h.id)).toEqual(g.ids);
    expect(r.body.results.map((h: { score: number }) => h.score)).toEqual(g.scores);
  });
});
