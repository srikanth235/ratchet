import { describe, expect, it } from "vitest";
import { generateCatalog } from "../../src/data.js";
import { createRng } from "../../src/rng.js";

describe("rng", () => {
  it("is deterministic and in [0,1)", () => {
    const a = createRng(7);
    const b = createRng(7);
    for (let i = 0; i < 100; i++) {
      const x = a();
      expect(x).toBe(b());
      expect(x).toBeGreaterThanOrEqual(0);
      expect(x).toBeLessThan(1);
    }
  });
});

describe("generateCatalog", () => {
  it("is deterministic for a seed and differs across seeds", () => {
    expect(generateCatalog(500, 1).products).toEqual(generateCatalog(500, 1).products);
    expect(generateCatalog(500, 1).products).not.toEqual(generateCatalog(500, 2).products);
  });

  it("generates well-formed products with sequential ids", () => {
    const { products } = generateCatalog(2000);
    products.forEach((p, i) => {
      expect(p.id).toBe(i + 1);
      expect(p.price).toBeGreaterThanOrEqual(5);
      expect(p.rating).toBeGreaterThanOrEqual(2);
      expect(p.rating).toBeLessThanOrEqual(5);
      expect(p.tags.length).toBeGreaterThanOrEqual(2);
      expect(new Set(p.tags).size).toBe(p.tags.length);
      expect(p.name.toLowerCase().startsWith(p.brand)).toBe(true);
    });
  });

  it("indexes name and tag words", () => {
    const catalog = generateCatalog(300);
    const first = catalog.products[0]!;
    expect(catalog.index.get(first.brand)?.name).toContain(0);
    expect(catalog.index.get(first.tags[0]!)?.tags).toContain(0);
  });
});
