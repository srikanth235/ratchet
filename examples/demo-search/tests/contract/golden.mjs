// Regenerate golden.json from the reference implementation:
//   pnpm build && node tests/contract/golden.mjs
import { writeFileSync } from "node:fs";
import { generateCatalog } from "../../dist/src/data.js";
import { search } from "../../dist/src/search.js";
import { GOLDEN_QUERIES } from "./golden-queries.mjs";

const catalog = generateCatalog(50_000, 42);
const golden = GOLDEN_QUERIES.map((query) => {
  const r = search(catalog, { limit: 20, offset: 0, ...query });
  return { query, total: r.total, ids: r.results.map((h) => h.id), scores: r.results.map((h) => h.score) };
});
writeFileSync(new URL("./golden.json", import.meta.url), JSON.stringify(golden, null, 1) + "\n");
console.log(`wrote ${golden.length} golden queries`);
