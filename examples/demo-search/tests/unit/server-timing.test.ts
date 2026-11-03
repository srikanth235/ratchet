import type { AddressInfo } from "node:net";
import type http from "node:http";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createServer } from "../../src/server.js";

let server: http.Server;
let base: string;
beforeAll(async () => {
  server = createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterAll(() => new Promise<void>((resolve) => server.close(() => resolve())));

describe("Server-Timing (read by bench/local.ts for the per-stage breakdown)", () => {
  it("reports parse, search and serialize durations for a search", async () => {
    const res = await fetch(`${base}/search?q=kettle&filters=price:<100`);
    expect(res.status).toBe(200);
    const header = res.headers.get("server-timing") ?? "";
    const spans = Object.fromEntries(header.split(",").map((e) => {
      const [name, d] = e.trim().split(";dur=");
      return [name, Number(d)];
    }));
    expect(Object.keys(spans)).toEqual(["parse", "search", "serialize"]);
    for (const v of Object.values(spans)) expect(v).toBeGreaterThanOrEqual(0);
  });

  it("is not sent on errors", async () => {
    const res = await fetch(`${base}/search?limit=abc`);
    expect(res.status).toBe(400);
    expect(res.headers.get("server-timing")).toBeNull();
  });
});
