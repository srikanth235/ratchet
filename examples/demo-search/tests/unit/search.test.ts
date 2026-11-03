import { beforeAll, describe, expect, it } from "vitest";
import { generateCatalog, type Catalog } from "../../src/data.js";
import { evaluateFilter, parseFilter } from "../../src/filter.js";
import { search, tokenizeQuery } from "../../src/search.js";

let catalog: Catalog;
beforeAll(() => {
  catalog = generateCatalog(3000, 5);
});
const run = (q?: string, filters?: string, limit = 20, offset = 0) => {
  const params: Parameters<typeof search>[1] = { limit, offset };
  if (q !== undefined) params.q = q;
  if (filters !== undefined) params.filters = filters;
  return search(catalog, params);
};

describe("tokenizeQuery", () => {
  it("lowercases and splits on non-alphanumerics", () => {
    expect(tokenizeQuery("  Sleek,  KETTLE-2 ")).toEqual(["sleek", "kettle", "2"]);
    expect(tokenizeQuery("")).toEqual([]);
  });
});

describe("search", () => {
  it("matches every product when there is no query or filter", () => {
    const r = run();
    expect(r.total).toBe(3000);
    expect(r.results).toHaveLength(20);
    expect(r.results.every((h) => h.score === 0)).toBe(true);
  });

  it("requires all terms to match a word prefix and scores name 3 / tag 1", () => {
    const r = run("kett");
    expect(r.total).toBeGreaterThan(0);
    for (const h of r.results) {
      const inName = h.name.toLowerCase().split(" ").some((w) => w.startsWith("kett"));
      const inTags = h.tags.some((t) => t.startsWith("kett"));
      expect(inName || inTags).toBe(true);
      expect(h.score).toBe(inName ? 3 : 1);
    }
    const two = run("acme kettle");
    for (const h of two.results) expect(h.name.toLowerCase()).toContain("kettle");
    expect(two.total).toBeLessThanOrEqual(run("kettle").total);
  });

  it("applies filters", () => {
    const r = run(undefined, "category:kitchen AND price:<50", 100);
    expect(r.total).toBeGreaterThan(0);
    for (const h of r.results) {
      expect(h.category).toBe("kitchen");
      expect(h.price).toBeLessThan(50);
    }
    const f = parseFilter("category:kitchen AND price:<50");
    expect(r.total).toBe(catalog.products.filter((p) => evaluateFilter(f, p)).length);
  });

  it("orders by score, rating, name, id", () => {
    const r = run("sleek", undefined, 100);
    for (let i = 1; i < r.results.length; i++) {
      const a = r.results[i - 1]!;
      const b = r.results[i]!;
      const ordered =
        a.score > b.score ||
        (a.score === b.score && (a.rating > b.rating ||
          (a.rating === b.rating && (a.name.localeCompare(b.name) < 0 ||
            (a.name === b.name && a.id < b.id)))));
      expect(ordered).toBe(true);
    }
  });

  it("pages consistently", () => {
    const all = run("compact", undefined, 100).results.map((h) => h.id);
    const page2 = run("compact", undefined, 10, 10).results.map((h) => h.id);
    expect(page2).toEqual(all.slice(10, 20));
    expect(run("compact", undefined, 10, 100000).results).toEqual([]);
  });

  it("does not mutate the shared catalog", () => {
    const before = JSON.stringify(catalog.products.slice(0, 50));
    run("compact");
    expect(JSON.stringify(catalog.products.slice(0, 50))).toBe(before);
  });
});
