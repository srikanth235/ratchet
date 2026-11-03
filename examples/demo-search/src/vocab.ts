export const CATEGORIES = [
  "electronics", "home", "kitchen", "outdoors", "toys", "books",
  "fashion", "sports", "beauty", "garden", "office", "automotive",
] as const;

/** Relative frequency of each category (same order as CATEGORIES). */
export const CATEGORY_WEIGHTS = [14, 12, 11, 8, 8, 7, 12, 8, 6, 5, 5, 4];

const BRAND_PREFIXES = [
  "acme", "zen", "nord", "kest", "lum", "orb", "pin", "quar",
  "rav", "sol", "tund", "umb", "vert", "wil", "xen", "yar",
];
const BRAND_SUFFIXES = ["", "ith", "ex", "ora", "ix"];
export const BRANDS: readonly string[] = BRAND_PREFIXES.flatMap((p) => BRAND_SUFFIXES.map((s) => p + s));

export const ADJECTIVES = [
  "compact", "rugged", "sleek", "classic", "modern", "premium", "lightweight", "durable",
  "ergonomic", "foldable", "smart", "silent", "rapid", "deluxe", "vintage", "minimal",
  "heavy", "portable", "wireless", "adjustable", "waterproof", "insulated", "stackable", "magnetic",
  "bold", "cozy", "sturdy", "elegant", "nimble", "radiant", "swift", "tidy",
  "vivid", "woven", "zesty", "airy", "brisk", "crisp", "dapper", "fluffy",
  "glossy", "handy", "jolly", "keen", "lofty", "mellow", "noble", "plush",
] as const;

export const NOUNS_BY_CATEGORY: Record<(typeof CATEGORIES)[number], readonly string[]> = {
  electronics: ["speaker", "headphones", "charger", "monitor", "keyboard", "camera", "router", "tablet"],
  home: ["lamp", "cushion", "curtain", "shelf", "rug", "mirror", "clock", "hanger"],
  kitchen: ["kettle", "blender", "skillet", "toaster", "grinder", "colander", "teapot", "whisk"],
  outdoors: ["tent", "backpack", "lantern", "hammock", "compass", "stove", "cooler", "tarp"],
  toys: ["puzzle", "robot", "blocks", "drone", "plush", "kite", "marbles", "rocket"],
  books: ["journal", "atlas", "novel", "cookbook", "almanac", "sketchbook", "notebook", "planner"],
  fashion: ["jacket", "scarf", "sneakers", "wallet", "gloves", "beanie", "belt", "sunglasses"],
  sports: ["racket", "helmet", "dumbbell", "bicycle", "goggles", "yogamat", "paddle", "skates"],
  beauty: ["serum", "lotion", "brush", "perfume", "balm", "shampoo", "mask", "mirror"],
  garden: ["trowel", "planter", "hose", "sprinkler", "shears", "fountain", "trellis", "rake"],
  office: ["stapler", "desk", "chair", "lamp", "binder", "marker", "organizer", "whiteboard"],
  automotive: ["wiper", "cover", "jack", "vacuum", "polish", "mat", "inflator", "organizer"],
};

export const TAGS = [
  "eco", "sale", "new", "bestseller", "gift", "outdoor", "indoor", "travel",
  "kids", "pro", "budget", "limited", "organic", "recycled", "handmade", "bundle",
  "steel", "bamboo", "leather", "cotton", "glass", "ceramic", "carbon", "aluminum",
  "solar", "rechargeable", "quiet", "fast", "compact", "family",
] as const;

export const DESCRIPTION_WORDS = [
  "built", "for", "everyday", "use", "with", "a", "reliable", "finish", "that", "lasts",
  "designed", "to", "fit", "any", "space", "and", "budget", "tested", "by", "experts",
  "ships", "in", "recyclable", "packaging", "backed", "two", "year", "warranty", "easy",
  "setup", "no", "tools", "required", "perfect", "gift", "home", "travel", "work", "play",
] as const;
