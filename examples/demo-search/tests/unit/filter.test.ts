import { describe, expect, it } from "vitest";
import type { Product } from "../../src/data.js";
import { evaluateFilter, FilterSyntaxError, parseFilter } from "../../src/filter.js";

const product: Product = {
  id: 1, name: "Acme Sleek Kettle", brand: "acme", category: "kitchen", price: 40, rating: 4.2,
  inStock: true, tags: ["eco", "steel"], description: "x",
};
const matches = (f: string, p: Product = product) => evaluateFilter(parseFilter(f), p);

describe("parseFilter", () => {
  it("parses conditions of every kind", () => {
    expect(parseFilter("category:Kitchen")).toEqual({ type: "text", field: "category", value: "kitchen" });
    expect(parseFilter("instock:false")).toEqual({ type: "bool", field: "instock", value: false });
    expect(parseFilter("price:<=50")).toEqual({ type: "num", field: "price", op: "lte", value: 50 });
    expect(parseFilter("rating:4.5")).toEqual({ type: "num", field: "rating", op: "eq", value: 4.5 });
    expect(parseFilter("price:10-50")).toEqual({ type: "range", field: "price", lo: 10, hi: 50 });
  });

  it("binds AND tighter than OR and honours parentheses", () => {
    expect(matches("brand:zenith AND price:<10 OR category:kitchen")).toBe(true);
    expect(matches("brand:zenith AND (price:<10 OR category:kitchen)")).toBe(false);
  });

  it("supports NOT and case-insensitive keywords", () => {
    expect(matches("not tag:sale and tag:eco")).toBe(true);
    expect(matches("NOT NOT brand:acme")).toBe(true);
  });

  it.each(["", "category", "category:", "bogus:1", "price:abc", "(category:home", "category:home)", "AND", "brand:a AND", "instock:maybe", "price:<"])(
    "rejects %j",
    (bad) => {
      expect(() => parseFilter(bad)).toThrow(FilterSyntaxError);
    },
  );
});

describe("evaluateFilter", () => {
  it.each([
    ["category:kitchen", true],
    ["category:home", false],
    ["tag:steel", true],
    ["tag:sale", false],
    ["price:40", true],
    ["price:<40", false],
    ["price:<=40", true],
    ["price:>40", false],
    ["price:>=40", true],
    ["price:40-50", true],
    ["price:41-50", false],
    ["rating:>=4.2", true],
    ["rating:>4.2", false],
    ["instock:true", true],
    ["instock:false", false],
  ])("%s -> %s", (f, expected) => {
    expect(matches(f)).toBe(expected);
  });
});
