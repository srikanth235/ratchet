// k6 benchmark: 500 rps constant arrival for 60 s over the query mix.
//
//   k6 run bench/k6/search.js
//   BASE_URL=http://127.0.0.1:3000 QUERY_MIX=/abs/path/mix.json RESULT_FILE=out.json k6 run bench/k6/search.js
//
// Writes { http_req_duration: {p50,p95,p99}, rps, error_rate } (ms) to RESULT_FILE.
import http from "k6/http";
import exec from "k6/execution";
import { check } from "k6";
import { SharedArray } from "k6/data";

const BASE_URL = __ENV.BASE_URL || "http://127.0.0.1:3000";
const RATE = Number(__ENV.RATE || 500);
const DURATION = __ENV.DURATION || "60s";

const queries = new SharedArray("queries", () => JSON.parse(open(__ENV.QUERY_MIX || "../query-mix.json")).queries);

export const options = {
  scenarios: {
    search: {
      executor: "constant-arrival-rate",
      rate: RATE,
      timeUnit: "1s",
      duration: DURATION,
      preAllocatedVUs: 50,
      maxVUs: 400,
    },
  },
  thresholds: { http_req_failed: ["rate<0.001"] },
  summaryTrendStats: ["med", "p(95)", "p(99)"],
};

export default function () {
  const q = queries[exec.scenario.iterationInTest % queries.length];
  const params = [];
  if (q.q) params.push("q=" + encodeURIComponent(q.q));
  if (q.filters) params.push("filters=" + encodeURIComponent(q.filters));
  if (q.limit) params.push("limit=" + q.limit);
  if (q.offset) params.push("offset=" + q.offset);
  const res = http.get(`${BASE_URL}/search?${params.join("&")}`);
  check(res, { "status is 200": (r) => r.status === 200 });
}

export function handleSummary(data) {
  const d = data.metrics.http_req_duration.values;
  const result = {
    http_req_duration: { p50: d.med, p95: d["p(95)"], p99: d["p(99)"] },
    rps: data.metrics.http_reqs.values.rate,
    error_rate: data.metrics.http_req_failed.values.rate,
    requests: data.metrics.http_reqs.values.count,
  };
  return { [__ENV.RESULT_FILE || "bench-result.json"]: JSON.stringify(result, null, 2) + "\n" };
}
