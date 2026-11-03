export const GOLDEN_QUERIES = [
  { q: "kettle" },
  { q: "sleek kettle" },
  { q: "acme" },
  { q: "zen", limit: 50 },
  { q: "eco", offset: 20 },
  { q: "rugged speaker", filters: "price:<100" },
  { q: "compact" , filters: "category:electronics AND (brand:acme OR brand:zenith) AND rating:>=3.5" },
  { filters: "brand:nordix AND rating:>=4 AND price:<150" },
  { filters: "category:garden AND NOT tag:sale AND price:20-60 AND instock:true", limit: 30 },
  { filters: "tag:solar AND brand:kestex AND price:<=300" },
  { q: "premium", filters: "tag:gift OR tag:travel", offset: 20 },
  { q: "zzzznomatch" },
];
