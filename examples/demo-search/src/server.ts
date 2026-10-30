import http from "node:http";
import { getCatalog, type Catalog } from "./data.js";
import { FilterSyntaxError } from "./filter.js";
import { search, type SearchParams } from "./search.js";

const MAX_Q = 200;
const MAX_FILTERS = 500;
const MAX_LIMIT = 100;
const MAX_OFFSET = 10_000;

function send(res: http.ServerResponse, status: number, body: unknown): void {
  const payload = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(payload),
  });
  res.end(payload);
}

function fail(res: http.ServerResponse, status: number, code: string, message: string): void {
  send(res, status, { error: { code, message } });
}

function intParam(raw: string | null, name: string, def: number, min: number, max: number): number | string {
  if (raw === null) return def;
  if (!/^\d+$/.test(raw)) return `${name} must be an integer`;
  const n = Number(raw);
  if (n < min || n > max) return `${name} must be between ${min} and ${max}`;
  return n;
}

function handleSearch(catalog: Catalog, url: URL, res: http.ServerResponse): void {
  const q = url.searchParams.get("q") ?? undefined;
  const filters = url.searchParams.get("filters") ?? undefined;
  if (q !== undefined && q.length > MAX_Q) return fail(res, 400, "invalid_request", `q must be at most ${MAX_Q} characters`);
  if (filters !== undefined && filters.length > MAX_FILTERS) {
    return fail(res, 400, "invalid_request", `filters must be at most ${MAX_FILTERS} characters`);
  }
  const limit = intParam(url.searchParams.get("limit"), "limit", 20, 1, MAX_LIMIT);
  if (typeof limit === "string") return fail(res, 400, "invalid_request", limit);
  const offset = intParam(url.searchParams.get("offset"), "offset", 0, 0, MAX_OFFSET);
  if (typeof offset === "string") return fail(res, 400, "invalid_request", offset);

  const params: SearchParams = { limit, offset };
  if (q) params.q = q;
  if (filters) params.filters = filters;
  try {
    send(res, 200, search(catalog, params));
  } catch (err) {
    if (err instanceof FilterSyntaxError) return fail(res, 400, "invalid_filters", err.message);
    throw err;
  }
}

export function createServer(catalog: Catalog = getCatalog()): http.Server {
  return http.createServer((req, res) => {
    try {
      const url = new URL(req.url ?? "/", "http://localhost");
      if (url.pathname !== "/search" && url.pathname !== "/health") {
        return fail(res, 404, "not_found", "no such route");
      }
      if (req.method !== "GET") return fail(res, 405, "method_not_allowed", "only GET is supported");
      if (url.pathname === "/health") {
        return send(res, 200, { status: "ok", products: catalog.products.length });
      }
      handleSearch(catalog, url, res);
    } catch {
      fail(res, 500, "internal_error", "internal error");
    }
  });
}
