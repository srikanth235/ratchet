/** The demo-search ratchet.yaml (examples/demo-search). Used by the dev engine when a request carries no config. */
export const DEFAULT_CONFIG_YAML = `version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: k6 run bench/k6/search.js
  repeats: 5
  hardware: c7i.2xlarge
  confirm_trace: s3://sealed/demo-search-trace.json
guardrails:
  - run: pnpm test:unit
  - run: pnpm test:contract
  - error_rate: "< 0.001"
  - peak_rss: "< 1.2x"
protected:
  - bench/**
  - ratchet.yaml
  - tests/contract/**
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.2xlarge, c7i.xlarge, m7i.large]
`;
