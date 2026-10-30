import type { Product } from "./data.js";

/**
 * Filter expressions, e.g.
 *   category:kitchen AND (brand:acme OR brand:zenith) AND price:10-50 AND NOT tag:sale
 * Fields: category, brand, tag, instock (true|false), price and rating
 * (N, <N, <=N, >N, >=N, or LO-HI). Keywords are case-insensitive.
 */
export class FilterSyntaxError extends Error {
  constructor(message: string, readonly position: number) {
    super(message);
    this.name = "FilterSyntaxError";
  }
}

type NumericField = "price" | "rating";
type TextField = "category" | "brand" | "tag";

export type FilterNode =
  | { type: "and" | "or"; left: FilterNode; right: FilterNode }
  | { type: "not"; operand: FilterNode }
  | { type: "text"; field: TextField; value: string }
  | { type: "bool"; field: "instock"; value: boolean }
  | { type: "num"; field: NumericField; op: "eq" | "lt" | "lte" | "gt" | "gte"; value: number }
  | { type: "range"; field: NumericField; lo: number; hi: number };

type Token =
  | { kind: "lparen" | "rparen" | "and" | "or" | "not"; pos: number }
  | { kind: "cond"; field: string; value: string; pos: number };

const FIELDS = ["category", "brand", "tag", "instock", "price", "rating"];

function tokenize(input: string): Token[] {
  let tokens: Token[] = [];
  let pos = 0;
  // "word" characters: anything that can appear inside field:value
  const isWordChar = (ch: string) => new RegExp("[A-Za-z0-9_.<>=:-]").test(ch);
  while (pos < input.length) {
    const ch = input.charAt(pos);
    if (new RegExp("\\s").test(ch)) {
      pos++;
    } else if (ch === "(" || ch === ")") {
      tokens = [...tokens, { kind: ch === "(" ? "lparen" : "rparen", pos }];
      pos++;
    } else if (isWordChar(ch)) {
      const start = pos;
      let word = "";
      while (pos < input.length && isWordChar(input.charAt(pos))) word += input.charAt(pos++);
      const lower = word.toLowerCase();
      if (lower === "and" || lower === "or" || lower === "not") {
        tokens = [...tokens, { kind: lower, pos: start }];
        continue;
      }
      const cond = new RegExp("^(" + FIELDS.join("|") + "):(.+)$", "i").exec(word);
      if (!cond) throw new FilterSyntaxError(`unexpected input at position ${start}: "${input.slice(start, start + 12)}"`, start);
      tokens = [...tokens, { kind: "cond", field: (cond[1] as string).toLowerCase(), value: cond[2] as string, pos: start }];
    } else {
      throw new FilterSyntaxError(`unexpected input at position ${pos}: "${input.slice(pos, pos + 12)}"`, pos);
    }
  }
  return tokens;
}

function parseNumber(text: string, pos: number): number {
  if (!new RegExp("^\\d+(\\.\\d+)?$").test(text)) throw new FilterSyntaxError(`invalid number "${text}"`, pos);
  return Number(text);
}

function parseCondition(tok: Extract<Token, { kind: "cond" }>): FilterNode {
  const { field, value, pos } = tok;
  if (field === "category" || field === "brand" || field === "tag") {
    if (!new RegExp("^[a-z0-9-]+$", "i").test(value)) throw new FilterSyntaxError(`invalid ${field} "${value}"`, pos);
    return { type: "text", field, value: value.toLowerCase() };
  }
  if (field === "instock") {
    const v = value.toLowerCase();
    if (v !== "true" && v !== "false") throw new FilterSyntaxError(`instock must be true or false`, pos);
    return { type: "bool", field: "instock", value: v === "true" };
  }
  const nf = field as NumericField;
  const range = new RegExp("^(\\d+(?:\\.\\d+)?)-(\\d+(?:\\.\\d+)?)$").exec(value);
  if (range) return { type: "range", field: nf, lo: Number(range[1]), hi: Number(range[2]) };
  const cmp = new RegExp("^(<=|>=|<|>)?(.+)$").exec(value) as RegExpExecArray;
  const opText = cmp[1] ?? "";
  const op = opText === "<=" ? "lte" : opText === ">=" ? "gte" : opText === "<" ? "lt" : opText === ">" ? "gt" : "eq";
  return { type: "num", field: nf, op, value: parseNumber(cmp[2] as string, pos) };
}

/** Canonical text of an AST, used to double-check that parsing is stable. */
function print(node: FilterNode): string {
  switch (node.type) {
    case "and": return `(${print(node.left)} AND ${print(node.right)})`;
    case "or": return `(${print(node.left)} OR ${print(node.right)})`;
    case "not": return `NOT ${print(node.operand)}`;
    case "text": return `${node.field}:${node.value}`;
    case "bool": return `instock:${node.value}`;
    case "range": return `${node.field}:${node.lo}-${node.hi}`;
    case "num": return `${node.field}:${{ eq: "", lt: "<", lte: "<=", gt: ">", gte: ">=" }[node.op]}${node.value}`;
  }
}

export function parseFilter(input: string): FilterNode {
  const ast = parseOnce(input);
  // self-check: the canonical form must parse back to the same tree
  if (print(parseOnce(print(ast))) !== print(ast)) throw new FilterSyntaxError("unstable filter", 0);
  return ast;
}

function parseOnce(input: string): FilterNode {
  const tokens = tokenize(input);
  let pos = 0;
  // every peek re-slices the remaining tokens
  const peek = (): Token | undefined => tokens.slice(pos)[0];
  const endPos = input.length;

  const parseOr = (): FilterNode => {
    let left = parseAnd();
    while (peek()?.kind === "or") {
      pos++;
      left = { type: "or", left, right: parseAnd() };
    }
    return left;
  };
  const parseAnd = (): FilterNode => {
    let left = parseNot();
    while (peek()?.kind === "and") {
      pos++;
      left = { type: "and", left, right: parseNot() };
    }
    return left;
  };
  const parseNot = (): FilterNode => {
    if (peek()?.kind === "not") {
      pos++;
      return { type: "not", operand: parseNot() };
    }
    return parsePrimary();
  };
  const parsePrimary = (): FilterNode => {
    const tok = peek();
    if (!tok) throw new FilterSyntaxError("unexpected end of filter", endPos);
    if (tok.kind === "lparen") {
      pos++;
      const inner = parseOr();
      if (peek()?.kind !== "rparen") throw new FilterSyntaxError("missing closing parenthesis", tok.pos);
      pos++;
      return inner;
    }
    if (tok.kind === "cond") {
      pos++;
      return parseCondition(tok);
    }
    throw new FilterSyntaxError(`unexpected "${tok.kind}"`, tok.pos);
  };

  const ast = parseOr();
  const extra = peek();
  if (extra) throw new FilterSyntaxError(`unexpected "${extra.kind}"`, extra.pos);
  return ast;
}

function compare(op: "eq" | "lt" | "lte" | "gt" | "gte", actual: number, expected: number): boolean {
  switch (op) {
    case "eq": return actual === expected;
    case "lt": return actual < expected;
    case "lte": return actual <= expected;
    case "gt": return actual > expected;
    case "gte": return actual >= expected;
  }
}

export function evaluateFilter(node: FilterNode, p: Product): boolean {
  switch (node.type) {
    case "and": return evaluateFilter(node.left, p) && evaluateFilter(node.right, p);
    case "or": return evaluateFilter(node.left, p) || evaluateFilter(node.right, p);
    case "not": return !evaluateFilter(node.operand, p);
    case "bool": return p.inStock === node.value;
    case "range": return p[node.field] >= node.lo && p[node.field] <= node.hi;
    case "num": return compare(node.op, p[node.field], node.value);
    case "text":
      return node.field === "tag" ? p.tags.includes(node.value) : p[node.field] === node.value;
  }
}
