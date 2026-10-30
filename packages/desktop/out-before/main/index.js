"use strict";
const fs = require("node:fs");
const node_path = require("node:path");
const zod = require("zod");
const yaml = require("yaml");
const crypto$1 = require("node:crypto");
const node_url = require("node:url");
const Client = require("better-sqlite3");
const electron = require("electron");
const COMPARATORS = ["<", "<=", ">", ">=", "=="];
const THRESHOLD_RE = /^\s*(<=|>=|==|<|>)\s*([+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)\s*(x?)\s*$/;
function parseThreshold(metric, raw) {
  const m = THRESHOLD_RE.exec(raw);
  if (!m) {
    return `invalid threshold ${JSON.stringify(raw)}: expected a comparator (${COMPARATORS.join(
      ", "
    )}) followed by a number, optionally suffixed with "x" for a multiple of baseline (e.g. "< 0.001" or "< 1.2x")`;
  }
  const value = Number(m[2]);
  if (!Number.isFinite(value)) return `invalid threshold ${JSON.stringify(raw)}: number is not finite`;
  const relative = m[3] === "x";
  if (relative && value <= 0) {
    return `invalid threshold ${JSON.stringify(raw)}: a baseline multiple must be greater than 0`;
  }
  return { kind: "threshold", metric, op: m[1], value, relative, raw: raw.trim() };
}
const guardrailSchema = zod.z.record(zod.z.string(), zod.z.string({ error: 'guardrail values must be strings (e.g. run: "pytest -q" or error_rate: "< 0.001")' }), {
  error: 'each guardrail must be a mapping with a single key, e.g. `run: pytest -q` or `error_rate: "< 0.001"`'
}).transform((rec, ctx) => {
  const keys = Object.keys(rec);
  const bail = (message, path = []) => {
    ctx.issues.push({ code: "custom", message, input: rec, path });
    return { kind: "command", run: "" };
  };
  if (keys.length !== 1) {
    return bail(
      `a guardrail must have exactly one key (\`run\` or a metric name), found ${keys.length}${keys.length ? `: ${keys.join(", ")}` : ""}`
    );
  }
  const key = keys[0];
  const value = rec[key];
  if (key === "run") {
    if (value.trim() === "") return bail("`run` must be a non-empty command", ["run"]);
    return { kind: "command", run: value };
  }
  const parsed = parseThreshold(key, value);
  if (typeof parsed === "string") return bail(parsed, [key]);
  return parsed;
});
const nonEmpty = (what) => zod.z.string({ error: `${what} must be a string` }).min(1, `${what} must not be empty`);
const configSchema = zod.z.strictObject({
  version: zod.z.literal(0, { error: "version must be 0 (the only supported ratchet.yaml version)" }),
  objective: zod.z.strictObject({
    metric: nonEmpty("objective.metric"),
    direction: zod.z.enum(["minimize", "maximize"], { error: 'direction must be "minimize" or "maximize"' }),
    min_gain: zod.z.number({ error: "min_gain must be a number (a fraction, e.g. 0.03 for 3%)" }).min(0, "min_gain must be >= 0").max(1, "min_gain is a fraction and must be <= 1 (0.03 means 3%)")
  }),
  bench: zod.z.strictObject({
    ref: nonEmpty("bench.ref"),
    run: nonEmpty("bench.run"),
    repeats: zod.z.number({ error: "repeats must be an integer" }).int("repeats must be an integer").min(1, "repeats must be >= 1"),
    hardware: nonEmpty("bench.hardware"),
    confirm_trace: zod.z.string({ error: "confirm_trace must be a string" }).min(1).optional()
  }),
  guardrails: zod.z.array(guardrailSchema, { error: "guardrails must be a list" }).default([]),
  protected: zod.z.array(nonEmpty("protected glob"), { error: "protected must be a list of globs" }).default([]),
  agent: zod.z.strictObject({
    instructions: nonEmpty("agent.instructions"),
    allowed_instance_types: zod.z.array(nonEmpty("instance type"), { error: "allowed_instance_types must be a list of strings" }).min(1, "allowed_instance_types must list at least one instance type")
  })
});
function formatPath(path) {
  let out = "";
  for (const seg of path) {
    if (typeof seg === "number") out += `[${seg}]`;
    else out += out ? `.${String(seg)}` : String(seg);
  }
  return out;
}
function parseConfig(text2) {
  const lineCounter = new yaml.LineCounter();
  const doc = yaml.parseDocument(text2, { prettyErrors: false, lineCounter });
  if (doc.errors.length > 0) {
    return {
      ok: false,
      errors: doc.errors.map((e) => {
        const line = e.pos?.[0] === void 0 ? void 0 : lineCounter.linePos(e.pos[0]);
        return {
          path: "",
          message: `invalid YAML${line ? ` (line ${line.line}, column ${line.col})` : ""}: ${e.message.split("\n")[0]}`
        };
      })
    };
  }
  const value = doc.toJS();
  if (value === null || value === void 0) {
    return { ok: false, errors: [{ path: "", message: "ratchet.yaml is empty" }] };
  }
  if (typeof value !== "object" || Array.isArray(value)) {
    return { ok: false, errors: [{ path: "", message: "ratchet.yaml must be a mapping at the top level" }] };
  }
  const res = configSchema.safeParse(value);
  if (res.success) return { ok: true, config: res.data };
  return {
    ok: false,
    errors: res.error.issues.map((i) => ({ path: formatPath(i.path), message: describeIssue(i) }))
  };
}
function describeIssue(i) {
  if (i.code === "unrecognized_keys" && i.keys) {
    return `unknown ${i.keys.length > 1 ? "keys" : "key"} ${i.keys.map((k) => JSON.stringify(k)).join(", ")}`;
  }
  if (i.code === "invalid_type" && i.input === void 0 && /expected .* received undefined/i.test(i.message)) {
    return "is required";
  }
  return i.message;
}
function assertSamples(samples) {
  if (samples.length === 0) throw new RangeError("at least one sample is required");
  for (const s of samples) {
    if (!Number.isFinite(s)) throw new RangeError(`samples must be finite numbers, got ${s}`);
  }
}
function mean(samples) {
  assertSamples(samples);
  let sum = 0;
  for (const s of samples) sum += s;
  return sum / samples.length;
}
function median$1(samples) {
  assertSamples(samples);
  const sorted = [...samples].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 === 1 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}
function stddev(samples) {
  assertSamples(samples);
  if (samples.length === 1) return 0;
  const m = mean(samples);
  let ss = 0;
  for (const s of samples) ss += (s - m) ** 2;
  return Math.sqrt(ss / (samples.length - 1));
}
function coefficientOfVariation(samples) {
  const sd = stddev(samples);
  if (sd === 0) return 0;
  const m = Math.abs(mean(samples));
  return m === 0 ? Infinity : sd / m;
}
function summarize(samples) {
  assertSamples(samples);
  return {
    n: samples.length,
    mean: mean(samples),
    median: median$1(samples),
    stddev: stddev(samples),
    cv: coefficientOfVariation(samples),
    min: Math.min(...samples),
    max: Math.max(...samples)
  };
}
function noiseFloor(baselineSamples, opts = {}) {
  const k = opts.k ?? 2;
  const minAbsolute = opts.minAbsolute ?? 0;
  if (!(k >= 0) || !Number.isFinite(k)) throw new RangeError("k must be a non-negative finite number");
  if (!(minAbsolute >= 0) || !Number.isFinite(minAbsolute)) {
    throw new RangeError("minAbsolute must be a non-negative finite number");
  }
  return Math.max(k * stddev(baselineSamples), minAbsolute);
}
function improvementOf(direction, best, value) {
  return direction === "minimize" ? best - value : value - best;
}
function checkFinite(name, v) {
  if (!Number.isFinite(v)) throw new RangeError(`${name} must be a finite number, got ${v}`);
}
function evaluateRatchet(input) {
  const { direction, minGain, noiseFloor: noiseFloor2, currentBest } = input;
  checkFinite("currentBest", currentBest);
  if (!(minGain >= 0) || !Number.isFinite(minGain)) throw new RangeError("minGain must be a finite number >= 0");
  if (!(noiseFloor2 >= 0) || !Number.isFinite(noiseFloor2)) {
    throw new RangeError("noiseFloor must be a finite number >= 0");
  }
  if (input.candidate.length === 0) throw new RangeError("candidate must have at least one sample");
  const candidateValue = median$1(input.candidate);
  const improvement = improvementOf(direction, currentBest, candidateValue);
  const requiredGain = Math.max(noiseFloor2, minGain * Math.abs(currentBest));
  const base = { candidateValue, improvement, requiredGain };
  const blockers = [];
  for (const g of input.guardrails) {
    if (!g.passed) blockers.push(`guardrail failed: ${g.name}${g.detail ? ` (${g.detail})` : ""}`);
  }
  for (const v of input.protectedViolations) {
    blockers.push(`protected path changed: ${v.path} (matches ${v.pattern})`);
  }
  if (blockers.length > 0) return { ...base, verdict: "blocked", reasons: blockers };
  const fmt = (n) => String(Number(n.toPrecision(6)));
  if (!(improvement > requiredGain)) {
    const why = improvement <= 0 ? `candidate ${fmt(candidateValue)} is not better than current best ${fmt(currentBest)} (${direction})` : `improvement ${fmt(improvement)} does not exceed required gain ${fmt(requiredGain)} (max of noise floor ${fmt(noiseFloor2)} and min_gain ${fmt(minGain * Math.abs(currentBest))})`;
    return { ...base, verdict: "rejected", reasons: [why] };
  }
  const win = `bench improvement ${fmt(improvement)} exceeds required gain ${fmt(requiredGain)}`;
  if (!input.confirm || input.confirm.length === 0) {
    return { ...base, verdict: "pending_confirm", reasons: [win, "confirm run on the hidden trace has not completed"] };
  }
  const confirmBest = input.confirmBest ?? currentBest;
  checkFinite("confirmBest", confirmBest);
  const confirmValue = median$1(input.confirm);
  const confirmImprovement = improvementOf(direction, confirmBest, confirmValue);
  if (!(confirmImprovement > 0)) {
    return {
      ...base,
      confirmImprovement,
      verdict: "failed_confirm",
      reasons: [
        win,
        `confirm run ${fmt(confirmValue)} does not beat current best ${fmt(confirmBest)} on the hidden trace (${direction})`
      ]
    };
  }
  return {
    ...base,
    confirmImprovement,
    verdict: "verified",
    reasons: [win, `confirm run ${fmt(confirmValue)} beats current best ${fmt(confirmBest)} on the hidden trace`]
  };
}
const entityKind = /* @__PURE__ */ Symbol.for("drizzle:entityKind");
function is(value, type) {
  if (!value || typeof value !== "object") {
    return false;
  }
  if (value instanceof type) {
    return true;
  }
  if (!Object.prototype.hasOwnProperty.call(type, entityKind)) {
    throw new Error(
      `Class "${type.name ?? "<unknown>"}" doesn't look like a Drizzle entity. If this is incorrect and the class is provided by Drizzle, please report this as a bug.`
    );
  }
  let cls = Object.getPrototypeOf(value).constructor;
  if (cls) {
    while (cls) {
      if (entityKind in cls && cls[entityKind] === type[entityKind]) {
        return true;
      }
      cls = Object.getPrototypeOf(cls);
    }
  }
  return false;
}
class Column {
  constructor(table, config) {
    this.table = table;
    this.config = config;
    this.name = config.name;
    this.keyAsName = config.keyAsName;
    this.notNull = config.notNull;
    this.default = config.default;
    this.defaultFn = config.defaultFn;
    this.onUpdateFn = config.onUpdateFn;
    this.hasDefault = config.hasDefault;
    this.primary = config.primaryKey;
    this.isUnique = config.isUnique;
    this.uniqueName = config.uniqueName;
    this.uniqueType = config.uniqueType;
    this.dataType = config.dataType;
    this.columnType = config.columnType;
    this.generated = config.generated;
    this.generatedIdentity = config.generatedIdentity;
  }
  static [entityKind] = "Column";
  name;
  keyAsName;
  primary;
  notNull;
  default;
  defaultFn;
  onUpdateFn;
  hasDefault;
  isUnique;
  uniqueName;
  uniqueType;
  dataType;
  columnType;
  enumValues = void 0;
  generated = void 0;
  generatedIdentity = void 0;
  config;
  mapFromDriverValue(value) {
    return value;
  }
  mapToDriverValue(value) {
    return value;
  }
  // ** @internal */
  shouldDisableInsert() {
    return this.config.generated !== void 0 && this.config.generated.type !== "byDefault";
  }
}
class ColumnBuilder {
  static [entityKind] = "ColumnBuilder";
  config;
  constructor(name, dataType, columnType) {
    this.config = {
      name,
      keyAsName: name === "",
      notNull: false,
      default: void 0,
      hasDefault: false,
      primaryKey: false,
      isUnique: false,
      uniqueName: void 0,
      uniqueType: void 0,
      dataType,
      columnType,
      generated: void 0
    };
  }
  /**
   * Changes the data type of the column. Commonly used with `json` columns. Also, useful for branded types.
   *
   * @example
   * ```ts
   * const users = pgTable('users', {
   * 	id: integer('id').$type<UserId>().primaryKey(),
   * 	details: json('details').$type<UserDetails>().notNull(),
   * });
   * ```
   */
  $type() {
    return this;
  }
  /**
   * Adds a `not null` clause to the column definition.
   *
   * Affects the `select` model of the table - columns *without* `not null` will be nullable on select.
   */
  notNull() {
    this.config.notNull = true;
    return this;
  }
  /**
   * Adds a `default <value>` clause to the column definition.
   *
   * Affects the `insert` model of the table - columns *with* `default` are optional on insert.
   *
   * If you need to set a dynamic default value, use {@link $defaultFn} instead.
   */
  default(value) {
    this.config.default = value;
    this.config.hasDefault = true;
    return this;
  }
  /**
   * Adds a dynamic default value to the column.
   * The function will be called when the row is inserted, and the returned value will be used as the column value.
   *
   * **Note:** This value does not affect the `drizzle-kit` behavior, it is only used at runtime in `drizzle-orm`.
   */
  $defaultFn(fn) {
    this.config.defaultFn = fn;
    this.config.hasDefault = true;
    return this;
  }
  /**
   * Alias for {@link $defaultFn}.
   */
  $default = this.$defaultFn;
  /**
   * Adds a dynamic update value to the column.
   * The function will be called when the row is updated, and the returned value will be used as the column value if none is provided.
   * If no `default` (or `$defaultFn`) value is provided, the function will be called when the row is inserted as well, and the returned value will be used as the column value.
   *
   * **Note:** This value does not affect the `drizzle-kit` behavior, it is only used at runtime in `drizzle-orm`.
   */
  $onUpdateFn(fn) {
    this.config.onUpdateFn = fn;
    this.config.hasDefault = true;
    return this;
  }
  /**
   * Alias for {@link $onUpdateFn}.
   */
  $onUpdate = this.$onUpdateFn;
  /**
   * Adds a `primary key` clause to the column definition. This implicitly makes the column `not null`.
   *
   * In SQLite, `integer primary key` implicitly makes the column auto-incrementing.
   */
  primaryKey() {
    this.config.primaryKey = true;
    this.config.notNull = true;
    return this;
  }
  /** @internal Sets the name of the column to the key within the table definition if a name was not given. */
  setName(name) {
    if (this.config.name !== "") return;
    this.config.name = name;
  }
}
const TableName = /* @__PURE__ */ Symbol.for("drizzle:Name");
const isPgEnumSym = /* @__PURE__ */ Symbol.for("drizzle:isPgEnum");
function isPgEnum(obj) {
  return !!obj && typeof obj === "function" && isPgEnumSym in obj && obj[isPgEnumSym] === true;
}
class Subquery {
  static [entityKind] = "Subquery";
  constructor(sql2, fields, alias, isWith = false, usedTables = []) {
    this._ = {
      brand: "Subquery",
      sql: sql2,
      selectedFields: fields,
      alias,
      isWith,
      usedTables
    };
  }
  // getSQL(): SQL<unknown> {
  // 	return new SQL([this]);
  // }
}
class WithSubquery extends Subquery {
  static [entityKind] = "WithSubquery";
}
const tracer = {
  startActiveSpan(name, fn) {
    {
      return fn();
    }
  }
};
const ViewBaseConfig = /* @__PURE__ */ Symbol.for("drizzle:ViewBaseConfig");
const Schema = /* @__PURE__ */ Symbol.for("drizzle:Schema");
const Columns = /* @__PURE__ */ Symbol.for("drizzle:Columns");
const ExtraConfigColumns = /* @__PURE__ */ Symbol.for("drizzle:ExtraConfigColumns");
const OriginalName = /* @__PURE__ */ Symbol.for("drizzle:OriginalName");
const BaseName = /* @__PURE__ */ Symbol.for("drizzle:BaseName");
const IsAlias = /* @__PURE__ */ Symbol.for("drizzle:IsAlias");
const ExtraConfigBuilder = /* @__PURE__ */ Symbol.for("drizzle:ExtraConfigBuilder");
const IsDrizzleTable = /* @__PURE__ */ Symbol.for("drizzle:IsDrizzleTable");
class Table {
  static [entityKind] = "Table";
  /** @internal */
  static Symbol = {
    Name: TableName,
    Schema,
    OriginalName,
    Columns,
    ExtraConfigColumns,
    BaseName,
    IsAlias,
    ExtraConfigBuilder
  };
  /**
   * @internal
   * Can be changed if the table is aliased.
   */
  [TableName];
  /**
   * @internal
   * Used to store the original name of the table, before any aliasing.
   */
  [OriginalName];
  /** @internal */
  [Schema];
  /** @internal */
  [Columns];
  /** @internal */
  [ExtraConfigColumns];
  /**
   *  @internal
   * Used to store the table name before the transformation via the `tableCreator` functions.
   */
  [BaseName];
  /** @internal */
  [IsAlias] = false;
  /** @internal */
  [IsDrizzleTable] = true;
  /** @internal */
  [ExtraConfigBuilder] = void 0;
  constructor(name, schema2, baseName) {
    this[TableName] = this[OriginalName] = name;
    this[Schema] = schema2;
    this[BaseName] = baseName;
  }
}
function getTableName(table) {
  return table[TableName];
}
function getTableUniqueName(table) {
  return `${table[Schema] ?? "public"}.${table[TableName]}`;
}
function isSQLWrapper(value) {
  return value !== null && value !== void 0 && typeof value.getSQL === "function";
}
function mergeQueries(queries) {
  const result = { sql: "", params: [] };
  for (const query of queries) {
    result.sql += query.sql;
    result.params.push(...query.params);
    if (query.typings?.length) {
      if (!result.typings) {
        result.typings = [];
      }
      result.typings.push(...query.typings);
    }
  }
  return result;
}
class StringChunk {
  static [entityKind] = "StringChunk";
  value;
  constructor(value) {
    this.value = Array.isArray(value) ? value : [value];
  }
  getSQL() {
    return new SQL([this]);
  }
}
class SQL {
  constructor(queryChunks) {
    this.queryChunks = queryChunks;
    for (const chunk of queryChunks) {
      if (is(chunk, Table)) {
        const schemaName = chunk[Table.Symbol.Schema];
        this.usedTables.push(
          schemaName === void 0 ? chunk[Table.Symbol.Name] : schemaName + "." + chunk[Table.Symbol.Name]
        );
      }
    }
  }
  static [entityKind] = "SQL";
  /** @internal */
  decoder = noopDecoder;
  shouldInlineParams = false;
  /** @internal */
  usedTables = [];
  append(query) {
    this.queryChunks.push(...query.queryChunks);
    return this;
  }
  toQuery(config) {
    return tracer.startActiveSpan("drizzle.buildSQL", (span) => {
      const query = this.buildQueryFromSourceParams(this.queryChunks, config);
      span?.setAttributes({
        "drizzle.query.text": query.sql,
        "drizzle.query.params": JSON.stringify(query.params)
      });
      return query;
    });
  }
  buildQueryFromSourceParams(chunks, _config) {
    const config = Object.assign({}, _config, {
      inlineParams: _config.inlineParams || this.shouldInlineParams,
      paramStartIndex: _config.paramStartIndex || { value: 0 }
    });
    const {
      casing,
      escapeName,
      escapeParam,
      prepareTyping,
      inlineParams,
      paramStartIndex
    } = config;
    return mergeQueries(chunks.map((chunk) => {
      if (is(chunk, StringChunk)) {
        return { sql: chunk.value.join(""), params: [] };
      }
      if (is(chunk, Name)) {
        return { sql: escapeName(chunk.value), params: [] };
      }
      if (chunk === void 0) {
        return { sql: "", params: [] };
      }
      if (Array.isArray(chunk)) {
        const result = [new StringChunk("(")];
        for (const [i, p] of chunk.entries()) {
          result.push(p);
          if (i < chunk.length - 1) {
            result.push(new StringChunk(", "));
          }
        }
        result.push(new StringChunk(")"));
        return this.buildQueryFromSourceParams(result, config);
      }
      if (is(chunk, SQL)) {
        return this.buildQueryFromSourceParams(chunk.queryChunks, {
          ...config,
          inlineParams: inlineParams || chunk.shouldInlineParams
        });
      }
      if (is(chunk, Table)) {
        const schemaName = chunk[Table.Symbol.Schema];
        const tableName = chunk[Table.Symbol.Name];
        return {
          sql: schemaName === void 0 || chunk[IsAlias] ? escapeName(tableName) : escapeName(schemaName) + "." + escapeName(tableName),
          params: []
        };
      }
      if (is(chunk, Column)) {
        const columnName = casing.getColumnCasing(chunk);
        if (_config.invokeSource === "indexes") {
          return { sql: escapeName(columnName), params: [] };
        }
        const schemaName = chunk.table[Table.Symbol.Schema];
        return {
          sql: chunk.table[IsAlias] || schemaName === void 0 ? escapeName(chunk.table[Table.Symbol.Name]) + "." + escapeName(columnName) : escapeName(schemaName) + "." + escapeName(chunk.table[Table.Symbol.Name]) + "." + escapeName(columnName),
          params: []
        };
      }
      if (is(chunk, View)) {
        const schemaName = chunk[ViewBaseConfig].schema;
        const viewName = chunk[ViewBaseConfig].name;
        return {
          sql: schemaName === void 0 || chunk[ViewBaseConfig].isAlias ? escapeName(viewName) : escapeName(schemaName) + "." + escapeName(viewName),
          params: []
        };
      }
      if (is(chunk, Param)) {
        if (is(chunk.value, Placeholder)) {
          return { sql: escapeParam(paramStartIndex.value++, chunk), params: [chunk], typings: ["none"] };
        }
        const mappedValue = chunk.value === null ? null : chunk.encoder.mapToDriverValue(chunk.value);
        if (is(mappedValue, SQL)) {
          return this.buildQueryFromSourceParams([mappedValue], config);
        }
        if (inlineParams) {
          return { sql: this.mapInlineParam(mappedValue, config), params: [] };
        }
        let typings = ["none"];
        if (prepareTyping) {
          typings = [prepareTyping(chunk.encoder)];
        }
        return { sql: escapeParam(paramStartIndex.value++, mappedValue), params: [mappedValue], typings };
      }
      if (is(chunk, Placeholder)) {
        return { sql: escapeParam(paramStartIndex.value++, chunk), params: [chunk], typings: ["none"] };
      }
      if (is(chunk, SQL.Aliased) && chunk.fieldAlias !== void 0) {
        return { sql: escapeName(chunk.fieldAlias), params: [] };
      }
      if (is(chunk, Subquery)) {
        if (chunk._.isWith) {
          return { sql: escapeName(chunk._.alias), params: [] };
        }
        return this.buildQueryFromSourceParams([
          new StringChunk("("),
          chunk._.sql,
          new StringChunk(") "),
          new Name(chunk._.alias)
        ], config);
      }
      if (isPgEnum(chunk)) {
        if (chunk.schema) {
          return { sql: escapeName(chunk.schema) + "." + escapeName(chunk.enumName), params: [] };
        }
        return { sql: escapeName(chunk.enumName), params: [] };
      }
      if (isSQLWrapper(chunk)) {
        if (chunk.shouldOmitSQLParens?.()) {
          return this.buildQueryFromSourceParams([chunk.getSQL()], config);
        }
        return this.buildQueryFromSourceParams([
          new StringChunk("("),
          chunk.getSQL(),
          new StringChunk(")")
        ], config);
      }
      if (inlineParams) {
        return { sql: this.mapInlineParam(chunk, config), params: [] };
      }
      return { sql: escapeParam(paramStartIndex.value++, chunk), params: [chunk], typings: ["none"] };
    }));
  }
  mapInlineParam(chunk, { escapeString }) {
    if (chunk === null) {
      return "null";
    }
    if (typeof chunk === "number" || typeof chunk === "boolean") {
      return chunk.toString();
    }
    if (typeof chunk === "string") {
      return escapeString(chunk);
    }
    if (typeof chunk === "object") {
      const mappedValueAsString = chunk.toString();
      if (mappedValueAsString === "[object Object]") {
        return escapeString(JSON.stringify(chunk));
      }
      return escapeString(mappedValueAsString);
    }
    throw new Error("Unexpected param value: " + chunk);
  }
  getSQL() {
    return this;
  }
  as(alias) {
    if (alias === void 0) {
      return this;
    }
    return new SQL.Aliased(this, alias);
  }
  mapWith(decoder) {
    this.decoder = typeof decoder === "function" ? { mapFromDriverValue: decoder } : decoder;
    return this;
  }
  inlineParams() {
    this.shouldInlineParams = true;
    return this;
  }
  /**
   * This method is used to conditionally include a part of the query.
   *
   * @param condition - Condition to check
   * @returns itself if the condition is `true`, otherwise `undefined`
   */
  if(condition) {
    return condition ? this : void 0;
  }
}
class Name {
  constructor(value) {
    this.value = value;
  }
  static [entityKind] = "Name";
  brand;
  getSQL() {
    return new SQL([this]);
  }
}
function isDriverValueEncoder(value) {
  return typeof value === "object" && value !== null && "mapToDriverValue" in value && typeof value.mapToDriverValue === "function";
}
const noopDecoder = {
  mapFromDriverValue: (value) => value
};
const noopEncoder = {
  mapToDriverValue: (value) => value
};
({
  ...noopDecoder,
  ...noopEncoder
});
class Param {
  /**
   * @param value - Parameter value
   * @param encoder - Encoder to convert the value to a driver parameter
   */
  constructor(value, encoder = noopEncoder) {
    this.value = value;
    this.encoder = encoder;
  }
  static [entityKind] = "Param";
  brand;
  getSQL() {
    return new SQL([this]);
  }
}
function sql(strings, ...params) {
  const queryChunks = [];
  if (params.length > 0 || strings.length > 0 && strings[0] !== "") {
    queryChunks.push(new StringChunk(strings[0]));
  }
  for (const [paramIndex, param2] of params.entries()) {
    queryChunks.push(param2, new StringChunk(strings[paramIndex + 1]));
  }
  return new SQL(queryChunks);
}
((sql2) => {
  function empty() {
    return new SQL([]);
  }
  sql2.empty = empty;
  function fromList(list) {
    return new SQL(list);
  }
  sql2.fromList = fromList;
  function raw(str) {
    return new SQL([new StringChunk(str)]);
  }
  sql2.raw = raw;
  function join(chunks, separator) {
    const result = [];
    for (const [i, chunk] of chunks.entries()) {
      if (i > 0 && separator !== void 0) {
        result.push(separator);
      }
      result.push(chunk);
    }
    return new SQL(result);
  }
  sql2.join = join;
  function identifier(value) {
    return new Name(value);
  }
  sql2.identifier = identifier;
  function placeholder2(name2) {
    return new Placeholder(name2);
  }
  sql2.placeholder = placeholder2;
  function param2(value, encoder) {
    return new Param(value, encoder);
  }
  sql2.param = param2;
})(sql || (sql = {}));
((SQL2) => {
  class Aliased {
    constructor(sql2, fieldAlias) {
      this.sql = sql2;
      this.fieldAlias = fieldAlias;
    }
    static [entityKind] = "SQL.Aliased";
    /** @internal */
    isSelectionField = false;
    getSQL() {
      return this.sql;
    }
    /** @internal */
    clone() {
      return new Aliased(this.sql, this.fieldAlias);
    }
  }
  SQL2.Aliased = Aliased;
})(SQL || (SQL = {}));
class Placeholder {
  constructor(name2) {
    this.name = name2;
  }
  static [entityKind] = "Placeholder";
  getSQL() {
    return new SQL([this]);
  }
}
function fillPlaceholders(params, values) {
  return params.map((p) => {
    if (is(p, Placeholder)) {
      if (!(p.name in values)) {
        throw new Error(`No value for placeholder "${p.name}" was provided`);
      }
      return values[p.name];
    }
    if (is(p, Param) && is(p.value, Placeholder)) {
      if (!(p.value.name in values)) {
        throw new Error(`No value for placeholder "${p.value.name}" was provided`);
      }
      return p.encoder.mapToDriverValue(values[p.value.name]);
    }
    return p;
  });
}
const IsDrizzleView = /* @__PURE__ */ Symbol.for("drizzle:IsDrizzleView");
class View {
  static [entityKind] = "View";
  /** @internal */
  [ViewBaseConfig];
  /** @internal */
  [IsDrizzleView] = true;
  constructor({ name: name2, schema: schema2, selectedFields, query }) {
    this[ViewBaseConfig] = {
      name: name2,
      originalName: name2,
      schema: schema2,
      selectedFields,
      query,
      isExisting: !query,
      isAlias: false
    };
  }
  getSQL() {
    return new SQL([this]);
  }
}
Column.prototype.getSQL = function() {
  return new SQL([this]);
};
Table.prototype.getSQL = function() {
  return new SQL([this]);
};
Subquery.prototype.getSQL = function() {
  return new SQL([this]);
};
class ColumnAliasProxyHandler {
  constructor(table) {
    this.table = table;
  }
  static [entityKind] = "ColumnAliasProxyHandler";
  get(columnObj, prop) {
    if (prop === "table") {
      return this.table;
    }
    return columnObj[prop];
  }
}
class TableAliasProxyHandler {
  constructor(alias, replaceOriginalName) {
    this.alias = alias;
    this.replaceOriginalName = replaceOriginalName;
  }
  static [entityKind] = "TableAliasProxyHandler";
  get(target, prop) {
    if (prop === Table.Symbol.IsAlias) {
      return true;
    }
    if (prop === Table.Symbol.Name) {
      return this.alias;
    }
    if (this.replaceOriginalName && prop === Table.Symbol.OriginalName) {
      return this.alias;
    }
    if (prop === ViewBaseConfig) {
      return {
        ...target[ViewBaseConfig],
        name: this.alias,
        isAlias: true
      };
    }
    if (prop === Table.Symbol.Columns) {
      const columns = target[Table.Symbol.Columns];
      if (!columns) {
        return columns;
      }
      const proxiedColumns = {};
      Object.keys(columns).map((key) => {
        proxiedColumns[key] = new Proxy(
          columns[key],
          new ColumnAliasProxyHandler(new Proxy(target, this))
        );
      });
      return proxiedColumns;
    }
    const value = target[prop];
    if (is(value, Column)) {
      return new Proxy(value, new ColumnAliasProxyHandler(new Proxy(target, this)));
    }
    return value;
  }
}
function aliasedTable(table, tableAlias) {
  return new Proxy(table, new TableAliasProxyHandler(tableAlias, false));
}
function aliasedTableColumn(column, tableAlias) {
  return new Proxy(
    column,
    new ColumnAliasProxyHandler(new Proxy(column.table, new TableAliasProxyHandler(tableAlias, false)))
  );
}
function mapColumnsInAliasedSQLToAlias(query, alias) {
  return new SQL.Aliased(mapColumnsInSQLToAlias(query.sql, alias), query.fieldAlias);
}
function mapColumnsInSQLToAlias(query, alias) {
  return sql.join(query.queryChunks.map((c) => {
    if (is(c, Column)) {
      return aliasedTableColumn(c, alias);
    }
    if (is(c, SQL)) {
      return mapColumnsInSQLToAlias(c, alias);
    }
    if (is(c, SQL.Aliased)) {
      return mapColumnsInAliasedSQLToAlias(c, alias);
    }
    return c;
  }));
}
class DrizzleError extends Error {
  static [entityKind] = "DrizzleError";
  constructor({ message, cause }) {
    super(message);
    this.name = "DrizzleError";
    this.cause = cause;
  }
}
class DrizzleQueryError extends Error {
  constructor(query, params, cause) {
    super(`Failed query: ${query}
params: ${params}`);
    this.query = query;
    this.params = params;
    this.cause = cause;
    Error.captureStackTrace(this, DrizzleQueryError);
    if (cause) this.cause = cause;
  }
}
class TransactionRollbackError extends DrizzleError {
  static [entityKind] = "TransactionRollbackError";
  constructor() {
    super({ message: "Rollback" });
  }
}
class ConsoleLogWriter {
  static [entityKind] = "ConsoleLogWriter";
  write(message) {
    console.log(message);
  }
}
class DefaultLogger {
  static [entityKind] = "DefaultLogger";
  writer;
  constructor(config) {
    this.writer = config?.writer ?? new ConsoleLogWriter();
  }
  logQuery(query, params) {
    const stringifiedParams = params.map((p) => {
      try {
        return JSON.stringify(p);
      } catch {
        return String(p);
      }
    });
    const paramsStr = stringifiedParams.length ? ` -- params: [${stringifiedParams.join(", ")}]` : "";
    this.writer.write(`Query: ${query}${paramsStr}`);
  }
}
class NoopLogger {
  static [entityKind] = "NoopLogger";
  logQuery() {
  }
}
class QueryPromise {
  static [entityKind] = "QueryPromise";
  [Symbol.toStringTag] = "QueryPromise";
  catch(onRejected) {
    return this.then(void 0, onRejected);
  }
  finally(onFinally) {
    return this.then(
      (value) => {
        onFinally?.();
        return value;
      },
      (reason) => {
        onFinally?.();
        throw reason;
      }
    );
  }
  then(onFulfilled, onRejected) {
    return this.execute().then(onFulfilled, onRejected);
  }
}
function mapResultRow(columns, row, joinsNotNullableMap) {
  const nullifyMap = {};
  const result = columns.reduce(
    (result2, { path, field }, columnIndex) => {
      let decoder;
      if (is(field, Column)) {
        decoder = field;
      } else if (is(field, SQL)) {
        decoder = field.decoder;
      } else if (is(field, Subquery)) {
        decoder = field._.sql.decoder;
      } else {
        decoder = field.sql.decoder;
      }
      let node = result2;
      for (const [pathChunkIndex, pathChunk] of path.entries()) {
        if (pathChunkIndex < path.length - 1) {
          if (!(pathChunk in node)) {
            node[pathChunk] = {};
          }
          node = node[pathChunk];
        } else {
          const rawValue = row[columnIndex];
          const value = node[pathChunk] = rawValue === null ? null : decoder.mapFromDriverValue(rawValue);
          if (joinsNotNullableMap && is(field, Column) && path.length === 2) {
            const objectName = path[0];
            if (!(objectName in nullifyMap)) {
              nullifyMap[objectName] = value === null ? getTableName(field.table) : false;
            } else if (typeof nullifyMap[objectName] === "string" && nullifyMap[objectName] !== getTableName(field.table)) {
              nullifyMap[objectName] = false;
            }
          }
        }
      }
      return result2;
    },
    {}
  );
  if (joinsNotNullableMap && Object.keys(nullifyMap).length > 0) {
    for (const [objectName, tableName] of Object.entries(nullifyMap)) {
      if (typeof tableName === "string" && !joinsNotNullableMap[tableName]) {
        result[objectName] = null;
      }
    }
  }
  return result;
}
function orderSelectedFields(fields, pathPrefix) {
  return Object.entries(fields).reduce((result, [name, field]) => {
    if (typeof name !== "string") {
      return result;
    }
    const newPath = pathPrefix ? [...pathPrefix, name] : [name];
    if (is(field, Column) || is(field, SQL) || is(field, SQL.Aliased) || is(field, Subquery)) {
      result.push({ path: newPath, field });
    } else if (is(field, Table)) {
      result.push(...orderSelectedFields(field[Table.Symbol.Columns], newPath));
    } else {
      result.push(...orderSelectedFields(field, newPath));
    }
    return result;
  }, []);
}
function haveSameKeys(left, right) {
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  if (leftKeys.length !== rightKeys.length) {
    return false;
  }
  for (const [index2, key] of leftKeys.entries()) {
    if (key !== rightKeys[index2]) {
      return false;
    }
  }
  return true;
}
function mapUpdateSet(table, values) {
  const entries = Object.entries(values).filter(([, value]) => value !== void 0).map(([key, value]) => {
    if (is(value, SQL) || is(value, Column)) {
      return [key, value];
    } else {
      return [key, new Param(value, table[Table.Symbol.Columns][key])];
    }
  });
  if (entries.length === 0) {
    throw new Error("No values to set");
  }
  return Object.fromEntries(entries);
}
function applyMixins(baseClass, extendedClasses) {
  for (const extendedClass of extendedClasses) {
    for (const name of Object.getOwnPropertyNames(extendedClass.prototype)) {
      if (name === "constructor") continue;
      Object.defineProperty(
        baseClass.prototype,
        name,
        Object.getOwnPropertyDescriptor(extendedClass.prototype, name) || /* @__PURE__ */ Object.create(null)
      );
    }
  }
}
function getTableColumns(table) {
  return table[Table.Symbol.Columns];
}
function getTableLikeName(table) {
  return is(table, Subquery) ? table._.alias : is(table, View) ? table[ViewBaseConfig].name : is(table, SQL) ? void 0 : table[Table.Symbol.IsAlias] ? table[Table.Symbol.Name] : table[Table.Symbol.BaseName];
}
function getColumnNameAndConfig(a, b) {
  return {
    name: typeof a === "string" && a.length > 0 ? a : "",
    config: typeof a === "object" ? a : b
  };
}
function isConfig(data) {
  if (typeof data !== "object" || data === null) return false;
  if (data.constructor.name !== "Object") return false;
  if ("logger" in data) {
    const type = typeof data["logger"];
    if (type !== "boolean" && (type !== "object" || typeof data["logger"]["logQuery"] !== "function") && type !== "undefined") return false;
    return true;
  }
  if ("schema" in data) {
    const type = typeof data["schema"];
    if (type !== "object" && type !== "undefined") return false;
    return true;
  }
  if ("casing" in data) {
    const type = typeof data["casing"];
    if (type !== "string" && type !== "undefined") return false;
    return true;
  }
  if ("mode" in data) {
    if (data["mode"] !== "default" || data["mode"] !== "planetscale" || data["mode"] !== void 0) return false;
    return true;
  }
  if ("connection" in data) {
    const type = typeof data["connection"];
    if (type !== "string" && type !== "object" && type !== "undefined") return false;
    return true;
  }
  if ("client" in data) {
    const type = typeof data["client"];
    if (type !== "object" && type !== "function" && type !== "undefined") return false;
    return true;
  }
  if (Object.keys(data).length === 0) return true;
  return false;
}
const textDecoder = typeof TextDecoder === "undefined" ? null : new TextDecoder();
const InlineForeignKeys$1 = /* @__PURE__ */ Symbol.for("drizzle:PgInlineForeignKeys");
const EnableRLS = /* @__PURE__ */ Symbol.for("drizzle:EnableRLS");
class PgTable extends Table {
  static [entityKind] = "PgTable";
  /** @internal */
  static Symbol = Object.assign({}, Table.Symbol, {
    InlineForeignKeys: InlineForeignKeys$1,
    EnableRLS
  });
  /**@internal */
  [InlineForeignKeys$1] = [];
  /** @internal */
  [EnableRLS] = false;
  /** @internal */
  [Table.Symbol.ExtraConfigBuilder] = void 0;
  /** @internal */
  [Table.Symbol.ExtraConfigColumns] = {};
}
class PrimaryKeyBuilder {
  static [entityKind] = "PgPrimaryKeyBuilder";
  /** @internal */
  columns;
  /** @internal */
  name;
  constructor(columns, name) {
    this.columns = columns;
    this.name = name;
  }
  /** @internal */
  build(table) {
    return new PrimaryKey(table, this.columns, this.name);
  }
}
class PrimaryKey {
  constructor(table, columns, name) {
    this.table = table;
    this.columns = columns;
    this.name = name;
  }
  static [entityKind] = "PgPrimaryKey";
  columns;
  name;
  getName() {
    return this.name ?? `${this.table[PgTable.Symbol.Name]}_${this.columns.map((column) => column.name).join("_")}_pk`;
  }
}
function bindIfParam(value, column) {
  if (isDriverValueEncoder(column) && !isSQLWrapper(value) && !is(value, Param) && !is(value, Placeholder) && !is(value, Column) && !is(value, Table) && !is(value, View)) {
    return new Param(value, column);
  }
  return value;
}
const eq = (left, right) => {
  return sql`${left} = ${bindIfParam(right, left)}`;
};
const ne = (left, right) => {
  return sql`${left} <> ${bindIfParam(right, left)}`;
};
function and(...unfilteredConditions) {
  const conditions = unfilteredConditions.filter(
    (c) => c !== void 0
  );
  if (conditions.length === 0) {
    return void 0;
  }
  if (conditions.length === 1) {
    return new SQL(conditions);
  }
  return new SQL([
    new StringChunk("("),
    sql.join(conditions, new StringChunk(" and ")),
    new StringChunk(")")
  ]);
}
function or(...unfilteredConditions) {
  const conditions = unfilteredConditions.filter(
    (c) => c !== void 0
  );
  if (conditions.length === 0) {
    return void 0;
  }
  if (conditions.length === 1) {
    return new SQL(conditions);
  }
  return new SQL([
    new StringChunk("("),
    sql.join(conditions, new StringChunk(" or ")),
    new StringChunk(")")
  ]);
}
function not(condition) {
  return sql`not ${condition}`;
}
const gt = (left, right) => {
  return sql`${left} > ${bindIfParam(right, left)}`;
};
const gte = (left, right) => {
  return sql`${left} >= ${bindIfParam(right, left)}`;
};
const lt = (left, right) => {
  return sql`${left} < ${bindIfParam(right, left)}`;
};
const lte = (left, right) => {
  return sql`${left} <= ${bindIfParam(right, left)}`;
};
function inArray(column, values) {
  if (Array.isArray(values)) {
    if (values.length === 0) {
      return sql`false`;
    }
    return sql`${column} in ${values.map((v) => bindIfParam(v, column))}`;
  }
  return sql`${column} in ${bindIfParam(values, column)}`;
}
function notInArray(column, values) {
  if (Array.isArray(values)) {
    if (values.length === 0) {
      return sql`true`;
    }
    return sql`${column} not in ${values.map((v) => bindIfParam(v, column))}`;
  }
  return sql`${column} not in ${bindIfParam(values, column)}`;
}
function isNull(value) {
  return sql`${value} is null`;
}
function isNotNull(value) {
  return sql`${value} is not null`;
}
function exists(subquery) {
  return sql`exists ${subquery}`;
}
function notExists(subquery) {
  return sql`not exists ${subquery}`;
}
function between(column, min, max) {
  return sql`${column} between ${bindIfParam(min, column)} and ${bindIfParam(
    max,
    column
  )}`;
}
function notBetween(column, min, max) {
  return sql`${column} not between ${bindIfParam(
    min,
    column
  )} and ${bindIfParam(max, column)}`;
}
function like(column, value) {
  return sql`${column} like ${value}`;
}
function notLike(column, value) {
  return sql`${column} not like ${value}`;
}
function ilike(column, value) {
  return sql`${column} ilike ${value}`;
}
function notIlike(column, value) {
  return sql`${column} not ilike ${value}`;
}
function asc(column) {
  return sql`${column} asc`;
}
function desc(column) {
  return sql`${column} desc`;
}
class Relation {
  constructor(sourceTable, referencedTable, relationName) {
    this.sourceTable = sourceTable;
    this.referencedTable = referencedTable;
    this.relationName = relationName;
    this.referencedTableName = referencedTable[Table.Symbol.Name];
  }
  static [entityKind] = "Relation";
  referencedTableName;
  fieldName;
}
class Relations {
  constructor(table, config) {
    this.table = table;
    this.config = config;
  }
  static [entityKind] = "Relations";
}
class One extends Relation {
  constructor(sourceTable, referencedTable, config, isNullable) {
    super(sourceTable, referencedTable, config?.relationName);
    this.config = config;
    this.isNullable = isNullable;
  }
  static [entityKind] = "One";
  withFieldName(fieldName) {
    const relation = new One(
      this.sourceTable,
      this.referencedTable,
      this.config,
      this.isNullable
    );
    relation.fieldName = fieldName;
    return relation;
  }
}
class Many extends Relation {
  constructor(sourceTable, referencedTable, config) {
    super(sourceTable, referencedTable, config?.relationName);
    this.config = config;
  }
  static [entityKind] = "Many";
  withFieldName(fieldName) {
    const relation = new Many(
      this.sourceTable,
      this.referencedTable,
      this.config
    );
    relation.fieldName = fieldName;
    return relation;
  }
}
function getOperators() {
  return {
    and,
    between,
    eq,
    exists,
    gt,
    gte,
    ilike,
    inArray,
    isNull,
    isNotNull,
    like,
    lt,
    lte,
    ne,
    not,
    notBetween,
    notExists,
    notLike,
    notIlike,
    notInArray,
    or,
    sql
  };
}
function getOrderByOperators() {
  return {
    sql,
    asc,
    desc
  };
}
function extractTablesRelationalConfig(schema2, configHelpers) {
  if (Object.keys(schema2).length === 1 && "default" in schema2 && !is(schema2["default"], Table)) {
    schema2 = schema2["default"];
  }
  const tableNamesMap = {};
  const relationsBuffer = {};
  const tablesConfig = {};
  for (const [key, value] of Object.entries(schema2)) {
    if (is(value, Table)) {
      const dbName = getTableUniqueName(value);
      const bufferedRelations = relationsBuffer[dbName];
      tableNamesMap[dbName] = key;
      tablesConfig[key] = {
        tsName: key,
        dbName: value[Table.Symbol.Name],
        schema: value[Table.Symbol.Schema],
        columns: value[Table.Symbol.Columns],
        relations: bufferedRelations?.relations ?? {},
        primaryKey: bufferedRelations?.primaryKey ?? []
      };
      for (const column of Object.values(
        value[Table.Symbol.Columns]
      )) {
        if (column.primary) {
          tablesConfig[key].primaryKey.push(column);
        }
      }
      const extraConfig = value[Table.Symbol.ExtraConfigBuilder]?.(value[Table.Symbol.ExtraConfigColumns]);
      if (extraConfig) {
        for (const configEntry of Object.values(extraConfig)) {
          if (is(configEntry, PrimaryKeyBuilder)) {
            tablesConfig[key].primaryKey.push(...configEntry.columns);
          }
        }
      }
    } else if (is(value, Relations)) {
      const dbName = getTableUniqueName(value.table);
      const tableName = tableNamesMap[dbName];
      const relations2 = value.config(
        configHelpers(value.table)
      );
      let primaryKey;
      for (const [relationName, relation] of Object.entries(relations2)) {
        if (tableName) {
          const tableConfig = tablesConfig[tableName];
          tableConfig.relations[relationName] = relation;
        } else {
          if (!(dbName in relationsBuffer)) {
            relationsBuffer[dbName] = {
              relations: {},
              primaryKey
            };
          }
          relationsBuffer[dbName].relations[relationName] = relation;
        }
      }
    }
  }
  return { tables: tablesConfig, tableNamesMap };
}
function createOne(sourceTable) {
  return function one(table, config) {
    return new One(
      sourceTable,
      table,
      config,
      config?.fields.reduce((res, f) => res && f.notNull, true) ?? false
    );
  };
}
function createMany(sourceTable) {
  return function many(referencedTable, config) {
    return new Many(sourceTable, referencedTable, config);
  };
}
function normalizeRelation(schema2, tableNamesMap, relation) {
  if (is(relation, One) && relation.config) {
    return {
      fields: relation.config.fields,
      references: relation.config.references
    };
  }
  const referencedTableTsName = tableNamesMap[getTableUniqueName(relation.referencedTable)];
  if (!referencedTableTsName) {
    throw new Error(
      `Table "${relation.referencedTable[Table.Symbol.Name]}" not found in schema`
    );
  }
  const referencedTableConfig = schema2[referencedTableTsName];
  if (!referencedTableConfig) {
    throw new Error(`Table "${referencedTableTsName}" not found in schema`);
  }
  const sourceTable = relation.sourceTable;
  const sourceTableTsName = tableNamesMap[getTableUniqueName(sourceTable)];
  if (!sourceTableTsName) {
    throw new Error(
      `Table "${sourceTable[Table.Symbol.Name]}" not found in schema`
    );
  }
  const reverseRelations = [];
  for (const referencedTableRelation of Object.values(
    referencedTableConfig.relations
  )) {
    if (relation.relationName && relation !== referencedTableRelation && referencedTableRelation.relationName === relation.relationName || !relation.relationName && referencedTableRelation.referencedTable === relation.sourceTable) {
      reverseRelations.push(referencedTableRelation);
    }
  }
  if (reverseRelations.length > 1) {
    throw relation.relationName ? new Error(
      `There are multiple relations with name "${relation.relationName}" in table "${referencedTableTsName}"`
    ) : new Error(
      `There are multiple relations between "${referencedTableTsName}" and "${relation.sourceTable[Table.Symbol.Name]}". Please specify relation name`
    );
  }
  if (reverseRelations[0] && is(reverseRelations[0], One) && reverseRelations[0].config) {
    return {
      fields: reverseRelations[0].config.references,
      references: reverseRelations[0].config.fields
    };
  }
  throw new Error(
    `There is not enough information to infer relation "${sourceTableTsName}.${relation.fieldName}"`
  );
}
function createTableRelationsHelpers(sourceTable) {
  return {
    one: createOne(sourceTable),
    many: createMany(sourceTable)
  };
}
function mapRelationalRow(tablesConfig, tableConfig, row, buildQueryResultSelection, mapColumnValue = (value) => value) {
  const result = {};
  for (const [
    selectionItemIndex,
    selectionItem
  ] of buildQueryResultSelection.entries()) {
    if (selectionItem.isJson) {
      const relation = tableConfig.relations[selectionItem.tsKey];
      const rawSubRows = row[selectionItemIndex];
      const subRows = typeof rawSubRows === "string" ? JSON.parse(rawSubRows) : rawSubRows;
      result[selectionItem.tsKey] = is(relation, One) ? subRows && mapRelationalRow(
        tablesConfig,
        tablesConfig[selectionItem.relationTableTsKey],
        subRows,
        selectionItem.selection,
        mapColumnValue
      ) : subRows.map(
        (subRow) => mapRelationalRow(
          tablesConfig,
          tablesConfig[selectionItem.relationTableTsKey],
          subRow,
          selectionItem.selection,
          mapColumnValue
        )
      );
    } else {
      const value = mapColumnValue(row[selectionItemIndex]);
      const field = selectionItem.field;
      let decoder;
      if (is(field, Column)) {
        decoder = field;
      } else if (is(field, SQL)) {
        decoder = field.decoder;
      } else {
        decoder = field.sql.decoder;
      }
      result[selectionItem.tsKey] = value === null ? null : decoder.mapFromDriverValue(value);
    }
  }
  return result;
}
class CheckBuilder {
  constructor(name, value) {
    this.name = name;
    this.value = value;
  }
  static [entityKind] = "SQLiteCheckBuilder";
  brand;
  build(table) {
    return new Check(table, this);
  }
}
class Check {
  constructor(table, builder) {
    this.table = table;
    this.name = builder.name;
    this.value = builder.value;
  }
  static [entityKind] = "SQLiteCheck";
  name;
  value;
}
function check(name, value) {
  return new CheckBuilder(name, value);
}
class ForeignKeyBuilder {
  static [entityKind] = "SQLiteForeignKeyBuilder";
  /** @internal */
  reference;
  /** @internal */
  _onUpdate;
  /** @internal */
  _onDelete;
  constructor(config, actions) {
    this.reference = () => {
      const { name, columns, foreignColumns } = config();
      return { name, columns, foreignTable: foreignColumns[0].table, foreignColumns };
    };
    if (actions) {
      this._onUpdate = actions.onUpdate;
      this._onDelete = actions.onDelete;
    }
  }
  onUpdate(action) {
    this._onUpdate = action;
    return this;
  }
  onDelete(action) {
    this._onDelete = action;
    return this;
  }
  /** @internal */
  build(table) {
    return new ForeignKey(table, this);
  }
}
class ForeignKey {
  constructor(table, builder) {
    this.table = table;
    this.reference = builder.reference;
    this.onUpdate = builder._onUpdate;
    this.onDelete = builder._onDelete;
  }
  static [entityKind] = "SQLiteForeignKey";
  reference;
  onUpdate;
  onDelete;
  getName() {
    const { name, columns, foreignColumns } = this.reference();
    const columnNames = columns.map((column) => column.name);
    const foreignColumnNames = foreignColumns.map((column) => column.name);
    const chunks = [
      this.table[TableName],
      ...columnNames,
      foreignColumns[0].table[TableName],
      ...foreignColumnNames
    ];
    return name ?? `${chunks.join("_")}_fk`;
  }
}
function uniqueKeyName(table, columns) {
  return `${table[TableName]}_${columns.join("_")}_unique`;
}
class SQLiteColumnBuilder extends ColumnBuilder {
  static [entityKind] = "SQLiteColumnBuilder";
  foreignKeyConfigs = [];
  references(ref, actions = {}) {
    this.foreignKeyConfigs.push({ ref, actions });
    return this;
  }
  unique(name) {
    this.config.isUnique = true;
    this.config.uniqueName = name;
    return this;
  }
  generatedAlwaysAs(as, config) {
    this.config.generated = {
      as,
      type: "always",
      mode: config?.mode ?? "virtual"
    };
    return this;
  }
  /** @internal */
  buildForeignKeys(column, table) {
    return this.foreignKeyConfigs.map(({ ref, actions }) => {
      return ((ref2, actions2) => {
        const builder = new ForeignKeyBuilder(() => {
          const foreignColumn = ref2();
          return { columns: [column], foreignColumns: [foreignColumn] };
        });
        if (actions2.onUpdate) {
          builder.onUpdate(actions2.onUpdate);
        }
        if (actions2.onDelete) {
          builder.onDelete(actions2.onDelete);
        }
        return builder.build(table);
      })(ref, actions);
    });
  }
}
class SQLiteColumn extends Column {
  constructor(table, config) {
    if (!config.uniqueName) {
      config.uniqueName = uniqueKeyName(table, [config.name]);
    }
    super(table, config);
    this.table = table;
  }
  static [entityKind] = "SQLiteColumn";
}
class SQLiteBigIntBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteBigIntBuilder";
  constructor(name) {
    super(name, "bigint", "SQLiteBigInt");
  }
  /** @internal */
  build(table) {
    return new SQLiteBigInt(table, this.config);
  }
}
class SQLiteBigInt extends SQLiteColumn {
  static [entityKind] = "SQLiteBigInt";
  getSQLType() {
    return "blob";
  }
  mapFromDriverValue(value) {
    if (typeof Buffer !== "undefined" && Buffer.from) {
      const buf = Buffer.isBuffer(value) ? value : value instanceof ArrayBuffer ? Buffer.from(value) : value.buffer ? Buffer.from(value.buffer, value.byteOffset, value.byteLength) : Buffer.from(value);
      return BigInt(buf.toString("utf8"));
    }
    return BigInt(textDecoder.decode(value));
  }
  mapToDriverValue(value) {
    return Buffer.from(value.toString());
  }
}
class SQLiteBlobJsonBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteBlobJsonBuilder";
  constructor(name) {
    super(name, "json", "SQLiteBlobJson");
  }
  /** @internal */
  build(table) {
    return new SQLiteBlobJson(
      table,
      this.config
    );
  }
}
class SQLiteBlobJson extends SQLiteColumn {
  static [entityKind] = "SQLiteBlobJson";
  getSQLType() {
    return "blob";
  }
  mapFromDriverValue(value) {
    if (typeof Buffer !== "undefined" && Buffer.from) {
      const buf = Buffer.isBuffer(value) ? value : value instanceof ArrayBuffer ? Buffer.from(value) : value.buffer ? Buffer.from(value.buffer, value.byteOffset, value.byteLength) : Buffer.from(value);
      return JSON.parse(buf.toString("utf8"));
    }
    return JSON.parse(textDecoder.decode(value));
  }
  mapToDriverValue(value) {
    return Buffer.from(JSON.stringify(value));
  }
}
class SQLiteBlobBufferBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteBlobBufferBuilder";
  constructor(name) {
    super(name, "buffer", "SQLiteBlobBuffer");
  }
  /** @internal */
  build(table) {
    return new SQLiteBlobBuffer(table, this.config);
  }
}
class SQLiteBlobBuffer extends SQLiteColumn {
  static [entityKind] = "SQLiteBlobBuffer";
  mapFromDriverValue(value) {
    if (Buffer.isBuffer(value)) {
      return value;
    }
    return Buffer.from(value);
  }
  getSQLType() {
    return "blob";
  }
}
function blob(a, b) {
  const { name, config } = getColumnNameAndConfig(a, b);
  if (config?.mode === "json") {
    return new SQLiteBlobJsonBuilder(name);
  }
  if (config?.mode === "bigint") {
    return new SQLiteBigIntBuilder(name);
  }
  return new SQLiteBlobBufferBuilder(name);
}
class SQLiteCustomColumnBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteCustomColumnBuilder";
  constructor(name, fieldConfig, customTypeParams) {
    super(name, "custom", "SQLiteCustomColumn");
    this.config.fieldConfig = fieldConfig;
    this.config.customTypeParams = customTypeParams;
  }
  /** @internal */
  build(table) {
    return new SQLiteCustomColumn(
      table,
      this.config
    );
  }
}
class SQLiteCustomColumn extends SQLiteColumn {
  static [entityKind] = "SQLiteCustomColumn";
  sqlName;
  mapTo;
  mapFrom;
  constructor(table, config) {
    super(table, config);
    this.sqlName = config.customTypeParams.dataType(config.fieldConfig);
    this.mapTo = config.customTypeParams.toDriver;
    this.mapFrom = config.customTypeParams.fromDriver;
  }
  getSQLType() {
    return this.sqlName;
  }
  mapFromDriverValue(value) {
    return typeof this.mapFrom === "function" ? this.mapFrom(value) : value;
  }
  mapToDriverValue(value) {
    return typeof this.mapTo === "function" ? this.mapTo(value) : value;
  }
}
function customType(customTypeParams) {
  return (a, b) => {
    const { name, config } = getColumnNameAndConfig(a, b);
    return new SQLiteCustomColumnBuilder(
      name,
      config,
      customTypeParams
    );
  };
}
class SQLiteBaseIntegerBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteBaseIntegerBuilder";
  constructor(name, dataType, columnType) {
    super(name, dataType, columnType);
    this.config.autoIncrement = false;
  }
  primaryKey(config) {
    if (config?.autoIncrement) {
      this.config.autoIncrement = true;
    }
    this.config.hasDefault = true;
    return super.primaryKey();
  }
}
class SQLiteBaseInteger extends SQLiteColumn {
  static [entityKind] = "SQLiteBaseInteger";
  autoIncrement = this.config.autoIncrement;
  getSQLType() {
    return "integer";
  }
}
class SQLiteIntegerBuilder extends SQLiteBaseIntegerBuilder {
  static [entityKind] = "SQLiteIntegerBuilder";
  constructor(name) {
    super(name, "number", "SQLiteInteger");
  }
  build(table) {
    return new SQLiteInteger(
      table,
      this.config
    );
  }
}
class SQLiteInteger extends SQLiteBaseInteger {
  static [entityKind] = "SQLiteInteger";
}
class SQLiteTimestampBuilder extends SQLiteBaseIntegerBuilder {
  static [entityKind] = "SQLiteTimestampBuilder";
  constructor(name, mode) {
    super(name, "date", "SQLiteTimestamp");
    this.config.mode = mode;
  }
  /**
   * @deprecated Use `default()` with your own expression instead.
   *
   * Adds `DEFAULT (cast((julianday('now') - 2440587.5)*86400000 as integer))` to the column, which is the current epoch timestamp in milliseconds.
   */
  defaultNow() {
    return this.default(sql`(cast((julianday('now') - 2440587.5)*86400000 as integer))`);
  }
  build(table) {
    return new SQLiteTimestamp(
      table,
      this.config
    );
  }
}
class SQLiteTimestamp extends SQLiteBaseInteger {
  static [entityKind] = "SQLiteTimestamp";
  mode = this.config.mode;
  mapFromDriverValue(value) {
    if (this.config.mode === "timestamp") {
      return new Date(value * 1e3);
    }
    return new Date(value);
  }
  mapToDriverValue(value) {
    const unix = value.getTime();
    if (this.config.mode === "timestamp") {
      return Math.floor(unix / 1e3);
    }
    return unix;
  }
}
class SQLiteBooleanBuilder extends SQLiteBaseIntegerBuilder {
  static [entityKind] = "SQLiteBooleanBuilder";
  constructor(name, mode) {
    super(name, "boolean", "SQLiteBoolean");
    this.config.mode = mode;
  }
  build(table) {
    return new SQLiteBoolean(
      table,
      this.config
    );
  }
}
class SQLiteBoolean extends SQLiteBaseInteger {
  static [entityKind] = "SQLiteBoolean";
  mode = this.config.mode;
  mapFromDriverValue(value) {
    return Number(value) === 1;
  }
  mapToDriverValue(value) {
    return value ? 1 : 0;
  }
}
function integer(a, b) {
  const { name, config } = getColumnNameAndConfig(a, b);
  if (config?.mode === "timestamp" || config?.mode === "timestamp_ms") {
    return new SQLiteTimestampBuilder(name, config.mode);
  }
  if (config?.mode === "boolean") {
    return new SQLiteBooleanBuilder(name, config.mode);
  }
  return new SQLiteIntegerBuilder(name);
}
class SQLiteNumericBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteNumericBuilder";
  constructor(name) {
    super(name, "string", "SQLiteNumeric");
  }
  /** @internal */
  build(table) {
    return new SQLiteNumeric(
      table,
      this.config
    );
  }
}
class SQLiteNumeric extends SQLiteColumn {
  static [entityKind] = "SQLiteNumeric";
  mapFromDriverValue(value) {
    if (typeof value === "string") return value;
    return String(value);
  }
  getSQLType() {
    return "numeric";
  }
}
class SQLiteNumericNumberBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteNumericNumberBuilder";
  constructor(name) {
    super(name, "number", "SQLiteNumericNumber");
  }
  /** @internal */
  build(table) {
    return new SQLiteNumericNumber(
      table,
      this.config
    );
  }
}
class SQLiteNumericNumber extends SQLiteColumn {
  static [entityKind] = "SQLiteNumericNumber";
  mapFromDriverValue(value) {
    if (typeof value === "number") return value;
    return Number(value);
  }
  mapToDriverValue = String;
  getSQLType() {
    return "numeric";
  }
}
class SQLiteNumericBigIntBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteNumericBigIntBuilder";
  constructor(name) {
    super(name, "bigint", "SQLiteNumericBigInt");
  }
  /** @internal */
  build(table) {
    return new SQLiteNumericBigInt(
      table,
      this.config
    );
  }
}
class SQLiteNumericBigInt extends SQLiteColumn {
  static [entityKind] = "SQLiteNumericBigInt";
  mapFromDriverValue = BigInt;
  mapToDriverValue = String;
  getSQLType() {
    return "numeric";
  }
}
function numeric(a, b) {
  const { name, config } = getColumnNameAndConfig(a, b);
  const mode = config?.mode;
  return mode === "number" ? new SQLiteNumericNumberBuilder(name) : mode === "bigint" ? new SQLiteNumericBigIntBuilder(name) : new SQLiteNumericBuilder(name);
}
class SQLiteRealBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteRealBuilder";
  constructor(name) {
    super(name, "number", "SQLiteReal");
  }
  /** @internal */
  build(table) {
    return new SQLiteReal(table, this.config);
  }
}
class SQLiteReal extends SQLiteColumn {
  static [entityKind] = "SQLiteReal";
  getSQLType() {
    return "real";
  }
}
function real(name) {
  return new SQLiteRealBuilder(name ?? "");
}
class SQLiteTextBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteTextBuilder";
  constructor(name, config) {
    super(name, "string", "SQLiteText");
    this.config.enumValues = config.enum;
    this.config.length = config.length;
  }
  /** @internal */
  build(table) {
    return new SQLiteText(
      table,
      this.config
    );
  }
}
class SQLiteText extends SQLiteColumn {
  static [entityKind] = "SQLiteText";
  enumValues = this.config.enumValues;
  length = this.config.length;
  constructor(table, config) {
    super(table, config);
  }
  getSQLType() {
    return `text${this.config.length ? `(${this.config.length})` : ""}`;
  }
}
class SQLiteTextJsonBuilder extends SQLiteColumnBuilder {
  static [entityKind] = "SQLiteTextJsonBuilder";
  constructor(name) {
    super(name, "json", "SQLiteTextJson");
  }
  /** @internal */
  build(table) {
    return new SQLiteTextJson(
      table,
      this.config
    );
  }
}
class SQLiteTextJson extends SQLiteColumn {
  static [entityKind] = "SQLiteTextJson";
  getSQLType() {
    return "text";
  }
  mapFromDriverValue(value) {
    return JSON.parse(value);
  }
  mapToDriverValue(value) {
    return JSON.stringify(value);
  }
}
function text(a, b = {}) {
  const { name, config } = getColumnNameAndConfig(a, b);
  if (config.mode === "json") {
    return new SQLiteTextJsonBuilder(name);
  }
  return new SQLiteTextBuilder(name, config);
}
class SelectionProxyHandler {
  static [entityKind] = "SelectionProxyHandler";
  config;
  constructor(config) {
    this.config = { ...config };
  }
  get(subquery, prop) {
    if (prop === "_") {
      return {
        ...subquery["_"],
        selectedFields: new Proxy(
          subquery._.selectedFields,
          this
        )
      };
    }
    if (prop === ViewBaseConfig) {
      return {
        ...subquery[ViewBaseConfig],
        selectedFields: new Proxy(
          subquery[ViewBaseConfig].selectedFields,
          this
        )
      };
    }
    if (typeof prop === "symbol") {
      return subquery[prop];
    }
    const columns = is(subquery, Subquery) ? subquery._.selectedFields : is(subquery, View) ? subquery[ViewBaseConfig].selectedFields : subquery;
    const value = columns[prop];
    if (is(value, SQL.Aliased)) {
      if (this.config.sqlAliasedBehavior === "sql" && !value.isSelectionField) {
        return value.sql;
      }
      const newValue = value.clone();
      newValue.isSelectionField = true;
      return newValue;
    }
    if (is(value, SQL)) {
      if (this.config.sqlBehavior === "sql") {
        return value;
      }
      throw new Error(
        `You tried to reference "${prop}" field from a subquery, which is a raw SQL field, but it doesn't have an alias declared. Please add an alias to the field using ".as('alias')" method.`
      );
    }
    if (is(value, Column)) {
      if (this.config.alias) {
        return new Proxy(
          value,
          new ColumnAliasProxyHandler(
            new Proxy(
              value.table,
              new TableAliasProxyHandler(this.config.alias, this.config.replaceOriginalName ?? false)
            )
          )
        );
      }
      return value;
    }
    if (typeof value !== "object" || value === null) {
      return value;
    }
    return new Proxy(value, new SelectionProxyHandler(this.config));
  }
}
function getSQLiteColumnBuilders() {
  return {
    blob,
    customType,
    integer,
    numeric,
    real,
    text
  };
}
const InlineForeignKeys = /* @__PURE__ */ Symbol.for("drizzle:SQLiteInlineForeignKeys");
class SQLiteTable extends Table {
  static [entityKind] = "SQLiteTable";
  /** @internal */
  static Symbol = Object.assign({}, Table.Symbol, {
    InlineForeignKeys
  });
  /** @internal */
  [Table.Symbol.Columns];
  /** @internal */
  [InlineForeignKeys] = [];
  /** @internal */
  [Table.Symbol.ExtraConfigBuilder] = void 0;
}
function sqliteTableBase(name, columns, extraConfig, schema2, baseName = name) {
  const rawTable = new SQLiteTable(name, schema2, baseName);
  const parsedColumns = typeof columns === "function" ? columns(getSQLiteColumnBuilders()) : columns;
  const builtColumns = Object.fromEntries(
    Object.entries(parsedColumns).map(([name2, colBuilderBase]) => {
      const colBuilder = colBuilderBase;
      colBuilder.setName(name2);
      const column = colBuilder.build(rawTable);
      rawTable[InlineForeignKeys].push(...colBuilder.buildForeignKeys(column, rawTable));
      return [name2, column];
    })
  );
  const table = Object.assign(rawTable, builtColumns);
  table[Table.Symbol.Columns] = builtColumns;
  table[Table.Symbol.ExtraConfigColumns] = builtColumns;
  if (extraConfig) {
    table[SQLiteTable.Symbol.ExtraConfigBuilder] = extraConfig;
  }
  return table;
}
const sqliteTable = (name, columns, extraConfig) => {
  return sqliteTableBase(name, columns, extraConfig);
};
class IndexBuilderOn {
  constructor(name, unique) {
    this.name = name;
    this.unique = unique;
  }
  static [entityKind] = "SQLiteIndexBuilderOn";
  on(...columns) {
    return new IndexBuilder(this.name, columns, this.unique);
  }
}
class IndexBuilder {
  static [entityKind] = "SQLiteIndexBuilder";
  /** @internal */
  config;
  constructor(name, columns, unique) {
    this.config = {
      name,
      columns,
      unique,
      where: void 0
    };
  }
  /**
   * Condition for partial index.
   */
  where(condition) {
    this.config.where = condition;
    return this;
  }
  /** @internal */
  build(table) {
    return new Index(this.config, table);
  }
}
class Index {
  static [entityKind] = "SQLiteIndex";
  config;
  constructor(config, table) {
    this.config = { ...config, table };
  }
}
function index(name) {
  return new IndexBuilderOn(name, false);
}
function uniqueIndex(name) {
  return new IndexBuilderOn(name, true);
}
function extractUsedTable(table) {
  if (is(table, SQLiteTable)) {
    return [`${table[Table.Symbol.BaseName]}`];
  }
  if (is(table, Subquery)) {
    return table._.usedTables ?? [];
  }
  if (is(table, SQL)) {
    return table.usedTables ?? [];
  }
  return [];
}
class SQLiteDeleteBase extends QueryPromise {
  constructor(table, session, dialect, withList) {
    super();
    this.table = table;
    this.session = session;
    this.dialect = dialect;
    this.config = { table, withList };
  }
  static [entityKind] = "SQLiteDelete";
  /** @internal */
  config;
  /**
   * Adds a `where` clause to the query.
   *
   * Calling this method will delete only those rows that fulfill a specified condition.
   *
   * See docs: {@link https://orm.drizzle.team/docs/delete}
   *
   * @param where the `where` clause.
   *
   * @example
   * You can use conditional operators and `sql function` to filter the rows to be deleted.
   *
   * ```ts
   * // Delete all cars with green color
   * db.delete(cars).where(eq(cars.color, 'green'));
   * // or
   * db.delete(cars).where(sql`${cars.color} = 'green'`)
   * ```
   *
   * You can logically combine conditional operators with `and()` and `or()` operators:
   *
   * ```ts
   * // Delete all BMW cars with a green color
   * db.delete(cars).where(and(eq(cars.color, 'green'), eq(cars.brand, 'BMW')));
   *
   * // Delete all cars with the green or blue color
   * db.delete(cars).where(or(eq(cars.color, 'green'), eq(cars.color, 'blue')));
   * ```
   */
  where(where) {
    this.config.where = where;
    return this;
  }
  orderBy(...columns) {
    if (typeof columns[0] === "function") {
      const orderBy = columns[0](
        new Proxy(
          this.config.table[Table.Symbol.Columns],
          new SelectionProxyHandler({ sqlAliasedBehavior: "alias", sqlBehavior: "sql" })
        )
      );
      const orderByArray = Array.isArray(orderBy) ? orderBy : [orderBy];
      this.config.orderBy = orderByArray;
    } else {
      const orderByArray = columns;
      this.config.orderBy = orderByArray;
    }
    return this;
  }
  limit(limit) {
    this.config.limit = limit;
    return this;
  }
  returning(fields = this.table[SQLiteTable.Symbol.Columns]) {
    this.config.returning = orderSelectedFields(fields);
    return this;
  }
  /** @internal */
  getSQL() {
    return this.dialect.buildDeleteQuery(this.config);
  }
  toSQL() {
    const { typings: _typings, ...rest } = this.dialect.sqlToQuery(this.getSQL());
    return rest;
  }
  /** @internal */
  _prepare(isOneTimeQuery = true) {
    return this.session[isOneTimeQuery ? "prepareOneTimeQuery" : "prepareQuery"](
      this.dialect.sqlToQuery(this.getSQL()),
      this.config.returning,
      this.config.returning ? "all" : "run",
      true,
      void 0,
      {
        type: "delete",
        tables: extractUsedTable(this.config.table)
      }
    );
  }
  prepare() {
    return this._prepare(false);
  }
  run = (placeholderValues) => {
    return this._prepare().run(placeholderValues);
  };
  all = (placeholderValues) => {
    return this._prepare().all(placeholderValues);
  };
  get = (placeholderValues) => {
    return this._prepare().get(placeholderValues);
  };
  values = (placeholderValues) => {
    return this._prepare().values(placeholderValues);
  };
  async execute(placeholderValues) {
    return this._prepare().execute(placeholderValues);
  }
  $dynamic() {
    return this;
  }
}
function toSnakeCase(input) {
  const words = input.replace(/['\u2019]/g, "").match(/[\da-z]+|[A-Z]+(?![a-z])|[A-Z][\da-z]+/g) ?? [];
  return words.map((word) => word.toLowerCase()).join("_");
}
function toCamelCase(input) {
  const words = input.replace(/['\u2019]/g, "").match(/[\da-z]+|[A-Z]+(?![a-z])|[A-Z][\da-z]+/g) ?? [];
  return words.reduce((acc, word, i) => {
    const formattedWord = i === 0 ? word.toLowerCase() : `${word[0].toUpperCase()}${word.slice(1)}`;
    return acc + formattedWord;
  }, "");
}
function noopCase(input) {
  return input;
}
class CasingCache {
  static [entityKind] = "CasingCache";
  /** @internal */
  cache = {};
  cachedTables = {};
  convert;
  constructor(casing) {
    this.convert = casing === "snake_case" ? toSnakeCase : casing === "camelCase" ? toCamelCase : noopCase;
  }
  getColumnCasing(column) {
    if (!column.keyAsName) return column.name;
    const schema2 = column.table[Table.Symbol.Schema] ?? "public";
    const tableName = column.table[Table.Symbol.OriginalName];
    const key = `${schema2}.${tableName}.${column.name}`;
    if (!this.cache[key]) {
      this.cacheTable(column.table);
    }
    return this.cache[key];
  }
  cacheTable(table) {
    const schema2 = table[Table.Symbol.Schema] ?? "public";
    const tableName = table[Table.Symbol.OriginalName];
    const tableKey = `${schema2}.${tableName}`;
    if (!this.cachedTables[tableKey]) {
      for (const column of Object.values(table[Table.Symbol.Columns])) {
        const columnKey = `${tableKey}.${column.name}`;
        this.cache[columnKey] = this.convert(column.name);
      }
      this.cachedTables[tableKey] = true;
    }
  }
  clearCache() {
    this.cache = {};
    this.cachedTables = {};
  }
}
class SQLiteViewBase extends View {
  static [entityKind] = "SQLiteViewBase";
}
class SQLiteDialect {
  static [entityKind] = "SQLiteDialect";
  /** @internal */
  casing;
  constructor(config) {
    this.casing = new CasingCache(config?.casing);
  }
  escapeName(name) {
    return `"${name.replace(/"/g, '""')}"`;
  }
  escapeParam(_num) {
    return "?";
  }
  escapeString(str) {
    return `'${str.replace(/'/g, "''")}'`;
  }
  buildWithCTE(queries) {
    if (!queries?.length) return void 0;
    const withSqlChunks = [sql`with `];
    for (const [i, w] of queries.entries()) {
      withSqlChunks.push(sql`${sql.identifier(w._.alias)} as (${w._.sql})`);
      if (i < queries.length - 1) {
        withSqlChunks.push(sql`, `);
      }
    }
    withSqlChunks.push(sql` `);
    return sql.join(withSqlChunks);
  }
  buildDeleteQuery({
    table,
    where,
    returning,
    withList,
    limit,
    orderBy
  }) {
    const withSql = this.buildWithCTE(withList);
    const returningSql = returning ? sql` returning ${this.buildSelection(returning, { isSingleTable: true })}` : void 0;
    const whereSql = where ? sql` where ${where}` : void 0;
    const orderBySql = this.buildOrderBy(orderBy);
    const limitSql = this.buildLimit(limit);
    return sql`${withSql}delete from ${table}${whereSql}${returningSql}${orderBySql}${limitSql}`;
  }
  buildUpdateSet(table, set) {
    const tableColumns = table[Table.Symbol.Columns];
    const columnNames = Object.keys(tableColumns).filter(
      (colName) => set[colName] !== void 0 || tableColumns[colName]?.onUpdateFn !== void 0
    );
    const setSize = columnNames.length;
    return sql.join(
      columnNames.flatMap((colName, i) => {
        const col = tableColumns[colName];
        const onUpdateFnResult = col.onUpdateFn?.();
        const value = set[colName] ?? (is(onUpdateFnResult, SQL) ? onUpdateFnResult : sql.param(onUpdateFnResult, col));
        const res = sql`${sql.identifier(this.casing.getColumnCasing(col))} = ${value}`;
        if (i < setSize - 1) {
          return [res, sql.raw(", ")];
        }
        return [res];
      })
    );
  }
  buildUpdateQuery({
    table,
    set,
    where,
    returning,
    withList,
    joins,
    from,
    limit,
    orderBy
  }) {
    const withSql = this.buildWithCTE(withList);
    const setSql = this.buildUpdateSet(table, set);
    const fromSql = from && sql.join([sql.raw(" from "), this.buildFromTable(from)]);
    const joinsSql = this.buildJoins(joins);
    const returningSql = returning ? sql` returning ${this.buildSelection(returning, { isSingleTable: true })}` : void 0;
    const whereSql = where ? sql` where ${where}` : void 0;
    const orderBySql = this.buildOrderBy(orderBy);
    const limitSql = this.buildLimit(limit);
    return sql`${withSql}update ${table} set ${setSql}${fromSql}${joinsSql}${whereSql}${returningSql}${orderBySql}${limitSql}`;
  }
  /**
   * Builds selection SQL with provided fields/expressions
   *
   * Examples:
   *
   * `select <selection> from`
   *
   * `insert ... returning <selection>`
   *
   * If `isSingleTable` is true, then columns won't be prefixed with table name
   */
  buildSelection(fields, { isSingleTable = false } = {}) {
    const columnsLen = fields.length;
    const chunks = fields.flatMap(({ field }, i) => {
      const chunk = [];
      if (is(field, SQL.Aliased) && field.isSelectionField) {
        chunk.push(sql.identifier(field.fieldAlias));
      } else if (is(field, SQL.Aliased) || is(field, SQL)) {
        const query = is(field, SQL.Aliased) ? field.sql : field;
        if (isSingleTable) {
          chunk.push(
            new SQL(
              query.queryChunks.map((c) => {
                if (is(c, Column)) {
                  return sql.identifier(this.casing.getColumnCasing(c));
                }
                return c;
              })
            )
          );
        } else {
          chunk.push(query);
        }
        if (is(field, SQL.Aliased)) {
          chunk.push(sql` as ${sql.identifier(field.fieldAlias)}`);
        }
      } else if (is(field, Column)) {
        const tableName = field.table[Table.Symbol.Name];
        if (field.columnType === "SQLiteNumericBigInt") {
          if (isSingleTable) {
            chunk.push(
              sql`cast(${sql.identifier(this.casing.getColumnCasing(field))} as text)`
            );
          } else {
            chunk.push(
              sql`cast(${sql.identifier(tableName)}.${sql.identifier(this.casing.getColumnCasing(field))} as text)`
            );
          }
        } else {
          if (isSingleTable) {
            chunk.push(sql.identifier(this.casing.getColumnCasing(field)));
          } else {
            chunk.push(
              sql`${sql.identifier(tableName)}.${sql.identifier(this.casing.getColumnCasing(field))}`
            );
          }
        }
      } else if (is(field, Subquery)) {
        const entries = Object.entries(field._.selectedFields);
        if (entries.length === 1) {
          const entry = entries[0][1];
          const fieldDecoder = is(entry, SQL) ? entry.decoder : is(entry, Column) ? { mapFromDriverValue: (v) => entry.mapFromDriverValue(v) } : entry.sql.decoder;
          if (fieldDecoder) field._.sql.decoder = fieldDecoder;
        }
        chunk.push(field);
      }
      if (i < columnsLen - 1) {
        chunk.push(sql`, `);
      }
      return chunk;
    });
    return sql.join(chunks);
  }
  buildJoins(joins) {
    if (!joins || joins.length === 0) {
      return void 0;
    }
    const joinsArray = [];
    if (joins) {
      for (const [index2, joinMeta] of joins.entries()) {
        if (index2 === 0) {
          joinsArray.push(sql` `);
        }
        const table = joinMeta.table;
        const onSql = joinMeta.on ? sql` on ${joinMeta.on}` : void 0;
        if (is(table, SQLiteTable)) {
          const tableName = table[SQLiteTable.Symbol.Name];
          const tableSchema = table[SQLiteTable.Symbol.Schema];
          const origTableName = table[SQLiteTable.Symbol.OriginalName];
          const alias = tableName === origTableName ? void 0 : joinMeta.alias;
          joinsArray.push(
            sql`${sql.raw(joinMeta.joinType)} join ${tableSchema ? sql`${sql.identifier(tableSchema)}.` : void 0}${sql.identifier(
              origTableName
            )}${alias && sql` ${sql.identifier(alias)}`}${onSql}`
          );
        } else {
          joinsArray.push(
            sql`${sql.raw(joinMeta.joinType)} join ${table}${onSql}`
          );
        }
        if (index2 < joins.length - 1) {
          joinsArray.push(sql` `);
        }
      }
    }
    return sql.join(joinsArray);
  }
  buildLimit(limit) {
    return typeof limit === "object" || typeof limit === "number" && limit >= 0 ? sql` limit ${limit}` : void 0;
  }
  buildOrderBy(orderBy) {
    const orderByList = [];
    if (orderBy) {
      for (const [index2, orderByValue] of orderBy.entries()) {
        orderByList.push(orderByValue);
        if (index2 < orderBy.length - 1) {
          orderByList.push(sql`, `);
        }
      }
    }
    return orderByList.length > 0 ? sql` order by ${sql.join(orderByList)}` : void 0;
  }
  buildFromTable(table) {
    if (is(table, Table) && table[Table.Symbol.IsAlias]) {
      return sql`${sql`${sql.identifier(table[Table.Symbol.Schema] ?? "")}.`.if(table[Table.Symbol.Schema])}${sql.identifier(
        table[Table.Symbol.OriginalName]
      )} ${sql.identifier(table[Table.Symbol.Name])}`;
    }
    return table;
  }
  buildSelectQuery({
    withList,
    fields,
    fieldsFlat,
    where,
    having,
    table,
    joins,
    orderBy,
    groupBy,
    limit,
    offset,
    distinct,
    setOperators
  }) {
    const fieldsList = fieldsFlat ?? orderSelectedFields(fields);
    for (const f of fieldsList) {
      if (is(f.field, Column) && getTableName(f.field.table) !== (is(table, Subquery) ? table._.alias : is(table, SQLiteViewBase) ? table[ViewBaseConfig].name : is(table, SQL) ? void 0 : getTableName(table)) && !((table2) => joins?.some(
        ({ alias }) => alias === (table2[Table.Symbol.IsAlias] ? getTableName(table2) : table2[Table.Symbol.BaseName])
      ))(f.field.table)) {
        const tableName = getTableName(f.field.table);
        throw new Error(
          `Your "${f.path.join(
            "->"
          )}" field references a column "${tableName}"."${f.field.name}", but the table "${tableName}" is not part of the query! Did you forget to join it?`
        );
      }
    }
    const isSingleTable = !joins || joins.length === 0;
    const withSql = this.buildWithCTE(withList);
    const distinctSql = distinct ? sql` distinct` : void 0;
    const selection = this.buildSelection(fieldsList, { isSingleTable });
    const tableSql = this.buildFromTable(table);
    const joinsSql = this.buildJoins(joins);
    const whereSql = where ? sql` where ${where}` : void 0;
    const havingSql = having ? sql` having ${having}` : void 0;
    const groupByList = [];
    if (groupBy) {
      for (const [index2, groupByValue] of groupBy.entries()) {
        groupByList.push(groupByValue);
        if (index2 < groupBy.length - 1) {
          groupByList.push(sql`, `);
        }
      }
    }
    const groupBySql = groupByList.length > 0 ? sql` group by ${sql.join(groupByList)}` : void 0;
    const orderBySql = this.buildOrderBy(orderBy);
    const limitSql = this.buildLimit(limit);
    const offsetSql = offset ? sql` offset ${offset}` : void 0;
    const finalQuery = sql`${withSql}select${distinctSql} ${selection} from ${tableSql}${joinsSql}${whereSql}${groupBySql}${havingSql}${orderBySql}${limitSql}${offsetSql}`;
    if (setOperators.length > 0) {
      return this.buildSetOperations(finalQuery, setOperators);
    }
    return finalQuery;
  }
  buildSetOperations(leftSelect, setOperators) {
    const [setOperator, ...rest] = setOperators;
    if (!setOperator) {
      throw new Error("Cannot pass undefined values to any set operator");
    }
    if (rest.length === 0) {
      return this.buildSetOperationQuery({ leftSelect, setOperator });
    }
    return this.buildSetOperations(
      this.buildSetOperationQuery({ leftSelect, setOperator }),
      rest
    );
  }
  buildSetOperationQuery({
    leftSelect,
    setOperator: { type, isAll, rightSelect, limit, orderBy, offset }
  }) {
    const leftChunk = sql`${leftSelect.getSQL()} `;
    const rightChunk = sql`${rightSelect.getSQL()}`;
    let orderBySql;
    if (orderBy && orderBy.length > 0) {
      const orderByValues = [];
      for (const singleOrderBy of orderBy) {
        if (is(singleOrderBy, SQLiteColumn)) {
          orderByValues.push(sql.identifier(singleOrderBy.name));
        } else if (is(singleOrderBy, SQL)) {
          for (let i = 0; i < singleOrderBy.queryChunks.length; i++) {
            const chunk = singleOrderBy.queryChunks[i];
            if (is(chunk, SQLiteColumn)) {
              singleOrderBy.queryChunks[i] = sql.identifier(
                this.casing.getColumnCasing(chunk)
              );
            }
          }
          orderByValues.push(sql`${singleOrderBy}`);
        } else {
          orderByValues.push(sql`${singleOrderBy}`);
        }
      }
      orderBySql = sql` order by ${sql.join(orderByValues, sql`, `)}`;
    }
    const limitSql = typeof limit === "object" || typeof limit === "number" && limit >= 0 ? sql` limit ${limit}` : void 0;
    const operatorChunk = sql.raw(`${type} ${isAll ? "all " : ""}`);
    const offsetSql = offset ? sql` offset ${offset}` : void 0;
    return sql`${leftChunk}${operatorChunk}${rightChunk}${orderBySql}${limitSql}${offsetSql}`;
  }
  buildInsertQuery({
    table,
    values: valuesOrSelect,
    onConflict,
    returning,
    withList,
    select
  }) {
    const valuesSqlList = [];
    const columns = table[Table.Symbol.Columns];
    const colEntries = Object.entries(columns).filter(
      ([_, col]) => !col.shouldDisableInsert()
    );
    const insertOrder = colEntries.map(([, column]) => sql.identifier(this.casing.getColumnCasing(column)));
    if (select) {
      const select2 = valuesOrSelect;
      if (is(select2, SQL)) {
        valuesSqlList.push(select2);
      } else {
        valuesSqlList.push(select2.getSQL());
      }
    } else {
      const values = valuesOrSelect;
      valuesSqlList.push(sql.raw("values "));
      for (const [valueIndex, value] of values.entries()) {
        const valueList = [];
        for (const [fieldName, col] of colEntries) {
          const colValue = value[fieldName];
          if (colValue === void 0 || is(colValue, Param) && colValue.value === void 0) {
            let defaultValue;
            if (col.default !== null && col.default !== void 0) {
              defaultValue = is(col.default, SQL) ? col.default : sql.param(col.default, col);
            } else if (col.defaultFn !== void 0) {
              const defaultFnResult = col.defaultFn();
              defaultValue = is(defaultFnResult, SQL) ? defaultFnResult : sql.param(defaultFnResult, col);
            } else if (!col.default && col.onUpdateFn !== void 0) {
              const onUpdateFnResult = col.onUpdateFn();
              defaultValue = is(onUpdateFnResult, SQL) ? onUpdateFnResult : sql.param(onUpdateFnResult, col);
            } else {
              defaultValue = sql`null`;
            }
            valueList.push(defaultValue);
          } else {
            valueList.push(colValue);
          }
        }
        valuesSqlList.push(valueList);
        if (valueIndex < values.length - 1) {
          valuesSqlList.push(sql`, `);
        }
      }
    }
    const withSql = this.buildWithCTE(withList);
    const valuesSql = sql.join(valuesSqlList);
    const returningSql = returning ? sql` returning ${this.buildSelection(returning, { isSingleTable: true })}` : void 0;
    const onConflictSql = onConflict?.length ? sql.join(onConflict) : void 0;
    return sql`${withSql}insert into ${table} ${insertOrder} ${valuesSql}${onConflictSql}${returningSql}`;
  }
  sqlToQuery(sql2, invokeSource) {
    return sql2.toQuery({
      casing: this.casing,
      escapeName: this.escapeName,
      escapeParam: this.escapeParam,
      escapeString: this.escapeString,
      invokeSource
    });
  }
  buildRelationalQuery({
    fullSchema,
    schema: schema2,
    tableNamesMap,
    table,
    tableConfig,
    queryConfig: config,
    tableAlias,
    nestedQueryRelation,
    joinOn
  }) {
    let selection = [];
    let limit, offset, orderBy = [], where;
    const joins = [];
    if (config === true) {
      const selectionEntries = Object.entries(tableConfig.columns);
      selection = selectionEntries.map(([key, value]) => ({
        dbKey: value.name,
        tsKey: key,
        field: aliasedTableColumn(value, tableAlias),
        relationTableTsKey: void 0,
        isJson: false,
        selection: []
      }));
    } else {
      const aliasedColumns = Object.fromEntries(
        Object.entries(tableConfig.columns).map(([key, value]) => [
          key,
          aliasedTableColumn(value, tableAlias)
        ])
      );
      if (config.where) {
        const whereSql = typeof config.where === "function" ? config.where(aliasedColumns, getOperators()) : config.where;
        where = whereSql && mapColumnsInSQLToAlias(whereSql, tableAlias);
      }
      const fieldsSelection = [];
      let selectedColumns = [];
      if (config.columns) {
        let isIncludeMode = false;
        for (const [field, value] of Object.entries(config.columns)) {
          if (value === void 0) {
            continue;
          }
          if (field in tableConfig.columns) {
            if (!isIncludeMode && value === true) {
              isIncludeMode = true;
            }
            selectedColumns.push(field);
          }
        }
        if (selectedColumns.length > 0) {
          selectedColumns = isIncludeMode ? selectedColumns.filter((c) => config.columns?.[c] === true) : Object.keys(tableConfig.columns).filter(
            (key) => !selectedColumns.includes(key)
          );
        }
      } else {
        selectedColumns = Object.keys(tableConfig.columns);
      }
      for (const field of selectedColumns) {
        const column = tableConfig.columns[field];
        fieldsSelection.push({ tsKey: field, value: column });
      }
      let selectedRelations = [];
      if (config.with) {
        selectedRelations = Object.entries(config.with).filter(
          (entry) => !!entry[1]
        ).map(([tsKey, queryConfig]) => ({
          tsKey,
          queryConfig,
          relation: tableConfig.relations[tsKey]
        }));
      }
      let extras;
      if (config.extras) {
        extras = typeof config.extras === "function" ? config.extras(aliasedColumns, { sql }) : config.extras;
        for (const [tsKey, value] of Object.entries(extras)) {
          fieldsSelection.push({
            tsKey,
            value: mapColumnsInAliasedSQLToAlias(value, tableAlias)
          });
        }
      }
      for (const { tsKey, value } of fieldsSelection) {
        selection.push({
          dbKey: is(value, SQL.Aliased) ? value.fieldAlias : tableConfig.columns[tsKey].name,
          tsKey,
          field: is(value, Column) ? aliasedTableColumn(value, tableAlias) : value,
          relationTableTsKey: void 0,
          isJson: false,
          selection: []
        });
      }
      let orderByOrig = typeof config.orderBy === "function" ? config.orderBy(aliasedColumns, getOrderByOperators()) : config.orderBy ?? [];
      if (!Array.isArray(orderByOrig)) {
        orderByOrig = [orderByOrig];
      }
      orderBy = orderByOrig.map((orderByValue) => {
        if (is(orderByValue, Column)) {
          return aliasedTableColumn(orderByValue, tableAlias);
        }
        return mapColumnsInSQLToAlias(orderByValue, tableAlias);
      });
      limit = config.limit;
      offset = config.offset;
      for (const {
        tsKey: selectedRelationTsKey,
        queryConfig: selectedRelationConfigValue,
        relation
      } of selectedRelations) {
        const normalizedRelation = normalizeRelation(
          schema2,
          tableNamesMap,
          relation
        );
        const relationTableName = getTableUniqueName(relation.referencedTable);
        const relationTableTsName = tableNamesMap[relationTableName];
        const relationTableAlias = `${tableAlias}_${selectedRelationTsKey}`;
        const joinOn2 = and(
          ...normalizedRelation.fields.map(
            (field2, i) => eq(
              aliasedTableColumn(
                normalizedRelation.references[i],
                relationTableAlias
              ),
              aliasedTableColumn(field2, tableAlias)
            )
          )
        );
        const builtRelation = this.buildRelationalQuery({
          fullSchema,
          schema: schema2,
          tableNamesMap,
          table: fullSchema[relationTableTsName],
          tableConfig: schema2[relationTableTsName],
          queryConfig: is(relation, One) ? selectedRelationConfigValue === true ? { limit: 1 } : { ...selectedRelationConfigValue, limit: 1 } : selectedRelationConfigValue,
          tableAlias: relationTableAlias,
          joinOn: joinOn2,
          nestedQueryRelation: relation
        });
        const field = sql`(${builtRelation.sql})`.as(selectedRelationTsKey);
        selection.push({
          dbKey: selectedRelationTsKey,
          tsKey: selectedRelationTsKey,
          field,
          relationTableTsKey: relationTableTsName,
          isJson: true,
          selection: builtRelation.selection
        });
      }
    }
    if (selection.length === 0) {
      throw new DrizzleError({
        message: `No fields selected for table "${tableConfig.tsName}" ("${tableAlias}"). You need to have at least one item in "columns", "with" or "extras". If you need to select all columns, omit the "columns" key or set it to undefined.`
      });
    }
    let result;
    where = and(joinOn, where);
    if (nestedQueryRelation) {
      let field = sql`json_array(${sql.join(
        selection.map(
          ({ field: field2 }) => is(field2, SQLiteColumn) ? sql.identifier(this.casing.getColumnCasing(field2)) : is(field2, SQL.Aliased) ? field2.sql : field2
        ),
        sql`, `
      )})`;
      if (is(nestedQueryRelation, Many)) {
        field = sql`coalesce(json_group_array(${field}), json_array())`;
      }
      const nestedSelection = [
        {
          dbKey: "data",
          tsKey: "data",
          field: field.as("data"),
          isJson: true,
          relationTableTsKey: tableConfig.tsName,
          selection
        }
      ];
      const needsSubquery = limit !== void 0 || offset !== void 0 || orderBy.length > 0;
      if (needsSubquery) {
        result = this.buildSelectQuery({
          table: aliasedTable(table, tableAlias),
          fields: {},
          fieldsFlat: [
            {
              path: [],
              field: sql.raw("*")
            }
          ],
          where,
          limit,
          offset,
          orderBy,
          setOperators: []
        });
        where = void 0;
        limit = void 0;
        offset = void 0;
        orderBy = void 0;
      } else {
        result = aliasedTable(table, tableAlias);
      }
      result = this.buildSelectQuery({
        table: is(result, SQLiteTable) ? result : new Subquery(result, {}, tableAlias),
        fields: {},
        fieldsFlat: nestedSelection.map(({ field: field2 }) => ({
          path: [],
          field: is(field2, Column) ? aliasedTableColumn(field2, tableAlias) : field2
        })),
        joins,
        where,
        limit,
        offset,
        orderBy,
        setOperators: []
      });
    } else {
      result = this.buildSelectQuery({
        table: aliasedTable(table, tableAlias),
        fields: {},
        fieldsFlat: selection.map(({ field }) => ({
          path: [],
          field: is(field, Column) ? aliasedTableColumn(field, tableAlias) : field
        })),
        joins,
        where,
        limit,
        offset,
        orderBy,
        setOperators: []
      });
    }
    return {
      tableTsKey: tableConfig.tsName,
      sql: result,
      selection
    };
  }
}
class SQLiteSyncDialect extends SQLiteDialect {
  static [entityKind] = "SQLiteSyncDialect";
  migrate(migrations, session, config) {
    const migrationsTable = config === void 0 ? "__drizzle_migrations" : typeof config === "string" ? "__drizzle_migrations" : config.migrationsTable ?? "__drizzle_migrations";
    const migrationTableCreate = sql`
			CREATE TABLE IF NOT EXISTS ${sql.identifier(migrationsTable)} (
				id SERIAL PRIMARY KEY,
				hash text NOT NULL,
				created_at numeric
			)
		`;
    session.run(migrationTableCreate);
    const dbMigrations = session.values(
      sql`SELECT id, hash, created_at FROM ${sql.identifier(migrationsTable)} ORDER BY created_at DESC LIMIT 1`
    );
    const lastDbMigration = dbMigrations[0] ?? void 0;
    session.run(sql`BEGIN`);
    try {
      for (const migration of migrations) {
        if (!lastDbMigration || Number(lastDbMigration[2]) < migration.folderMillis) {
          for (const stmt of migration.sql) {
            session.run(sql.raw(stmt));
          }
          session.run(
            sql`INSERT INTO ${sql.identifier(
              migrationsTable
            )} ("hash", "created_at") VALUES(${migration.hash}, ${migration.folderMillis})`
          );
        }
      }
      session.run(sql`COMMIT`);
    } catch (e) {
      session.run(sql`ROLLBACK`);
      throw e;
    }
  }
}
class TypedQueryBuilder {
  static [entityKind] = "TypedQueryBuilder";
  /** @internal */
  getSelectedFields() {
    return this._.selectedFields;
  }
}
class SQLiteSelectBuilder {
  static [entityKind] = "SQLiteSelectBuilder";
  fields;
  session;
  dialect;
  withList;
  distinct;
  constructor(config) {
    this.fields = config.fields;
    this.session = config.session;
    this.dialect = config.dialect;
    this.withList = config.withList;
    this.distinct = config.distinct;
  }
  from(source) {
    const isPartialSelect = !!this.fields;
    let fields;
    if (this.fields) {
      fields = this.fields;
    } else if (is(source, Subquery)) {
      fields = Object.fromEntries(
        Object.keys(source._.selectedFields).map((key) => [key, source[key]])
      );
    } else if (is(source, SQLiteViewBase)) {
      fields = source[ViewBaseConfig].selectedFields;
    } else if (is(source, SQL)) {
      fields = {};
    } else {
      fields = getTableColumns(source);
    }
    return new SQLiteSelectBase({
      table: source,
      fields,
      isPartialSelect,
      session: this.session,
      dialect: this.dialect,
      withList: this.withList,
      distinct: this.distinct
    });
  }
}
class SQLiteSelectQueryBuilderBase extends TypedQueryBuilder {
  static [entityKind] = "SQLiteSelectQueryBuilder";
  _;
  /** @internal */
  config;
  joinsNotNullableMap;
  tableName;
  isPartialSelect;
  session;
  dialect;
  cacheConfig = void 0;
  usedTables = /* @__PURE__ */ new Set();
  constructor({ table, fields, isPartialSelect, session, dialect, withList, distinct }) {
    super();
    this.config = {
      withList,
      table,
      fields: { ...fields },
      distinct,
      setOperators: []
    };
    this.isPartialSelect = isPartialSelect;
    this.session = session;
    this.dialect = dialect;
    this._ = {
      selectedFields: fields,
      config: this.config
    };
    this.tableName = getTableLikeName(table);
    this.joinsNotNullableMap = typeof this.tableName === "string" ? { [this.tableName]: true } : {};
    for (const item of extractUsedTable(table)) this.usedTables.add(item);
  }
  /** @internal */
  getUsedTables() {
    return [...this.usedTables];
  }
  createJoin(joinType) {
    return (table, on) => {
      const baseTableName = this.tableName;
      const tableName = getTableLikeName(table);
      for (const item of extractUsedTable(table)) this.usedTables.add(item);
      if (typeof tableName === "string" && this.config.joins?.some((join) => join.alias === tableName)) {
        throw new Error(`Alias "${tableName}" is already used in this query`);
      }
      if (!this.isPartialSelect) {
        if (Object.keys(this.joinsNotNullableMap).length === 1 && typeof baseTableName === "string") {
          this.config.fields = {
            [baseTableName]: this.config.fields
          };
        }
        if (typeof tableName === "string" && !is(table, SQL)) {
          const selection = is(table, Subquery) ? table._.selectedFields : is(table, View) ? table[ViewBaseConfig].selectedFields : table[Table.Symbol.Columns];
          this.config.fields[tableName] = selection;
        }
      }
      if (typeof on === "function") {
        on = on(
          new Proxy(
            this.config.fields,
            new SelectionProxyHandler({ sqlAliasedBehavior: "sql", sqlBehavior: "sql" })
          )
        );
      }
      if (!this.config.joins) {
        this.config.joins = [];
      }
      this.config.joins.push({ on, table, joinType, alias: tableName });
      if (typeof tableName === "string") {
        switch (joinType) {
          case "left": {
            this.joinsNotNullableMap[tableName] = false;
            break;
          }
          case "right": {
            this.joinsNotNullableMap = Object.fromEntries(
              Object.entries(this.joinsNotNullableMap).map(([key]) => [key, false])
            );
            this.joinsNotNullableMap[tableName] = true;
            break;
          }
          case "cross":
          case "inner": {
            this.joinsNotNullableMap[tableName] = true;
            break;
          }
          case "full": {
            this.joinsNotNullableMap = Object.fromEntries(
              Object.entries(this.joinsNotNullableMap).map(([key]) => [key, false])
            );
            this.joinsNotNullableMap[tableName] = false;
            break;
          }
        }
      }
      return this;
    };
  }
  /**
   * Executes a `left join` operation by adding another table to the current query.
   *
   * Calling this method associates each row of the table with the corresponding row from the joined table, if a match is found. If no matching row exists, it sets all columns of the joined table to null.
   *
   * See docs: {@link https://orm.drizzle.team/docs/joins#left-join}
   *
   * @param table the table to join.
   * @param on the `on` clause.
   *
   * @example
   *
   * ```ts
   * // Select all users and their pets
   * const usersWithPets: { user: User; pets: Pet | null; }[] = await db.select()
   *   .from(users)
   *   .leftJoin(pets, eq(users.id, pets.ownerId))
   *
   * // Select userId and petId
   * const usersIdsAndPetIds: { userId: number; petId: number | null; }[] = await db.select({
   *   userId: users.id,
   *   petId: pets.id,
   * })
   *   .from(users)
   *   .leftJoin(pets, eq(users.id, pets.ownerId))
   * ```
   */
  leftJoin = this.createJoin("left");
  /**
   * Executes a `right join` operation by adding another table to the current query.
   *
   * Calling this method associates each row of the joined table with the corresponding row from the main table, if a match is found. If no matching row exists, it sets all columns of the main table to null.
   *
   * See docs: {@link https://orm.drizzle.team/docs/joins#right-join}
   *
   * @param table the table to join.
   * @param on the `on` clause.
   *
   * @example
   *
   * ```ts
   * // Select all users and their pets
   * const usersWithPets: { user: User | null; pets: Pet; }[] = await db.select()
   *   .from(users)
   *   .rightJoin(pets, eq(users.id, pets.ownerId))
   *
   * // Select userId and petId
   * const usersIdsAndPetIds: { userId: number | null; petId: number; }[] = await db.select({
   *   userId: users.id,
   *   petId: pets.id,
   * })
   *   .from(users)
   *   .rightJoin(pets, eq(users.id, pets.ownerId))
   * ```
   */
  rightJoin = this.createJoin("right");
  /**
   * Executes an `inner join` operation, creating a new table by combining rows from two tables that have matching values.
   *
   * Calling this method retrieves rows that have corresponding entries in both joined tables. Rows without matching entries in either table are excluded, resulting in a table that includes only matching pairs.
   *
   * See docs: {@link https://orm.drizzle.team/docs/joins#inner-join}
   *
   * @param table the table to join.
   * @param on the `on` clause.
   *
   * @example
   *
   * ```ts
   * // Select all users and their pets
   * const usersWithPets: { user: User; pets: Pet; }[] = await db.select()
   *   .from(users)
   *   .innerJoin(pets, eq(users.id, pets.ownerId))
   *
   * // Select userId and petId
   * const usersIdsAndPetIds: { userId: number; petId: number; }[] = await db.select({
   *   userId: users.id,
   *   petId: pets.id,
   * })
   *   .from(users)
   *   .innerJoin(pets, eq(users.id, pets.ownerId))
   * ```
   */
  innerJoin = this.createJoin("inner");
  /**
   * Executes a `full join` operation by combining rows from two tables into a new table.
   *
   * Calling this method retrieves all rows from both main and joined tables, merging rows with matching values and filling in `null` for non-matching columns.
   *
   * See docs: {@link https://orm.drizzle.team/docs/joins#full-join}
   *
   * @param table the table to join.
   * @param on the `on` clause.
   *
   * @example
   *
   * ```ts
   * // Select all users and their pets
   * const usersWithPets: { user: User | null; pets: Pet | null; }[] = await db.select()
   *   .from(users)
   *   .fullJoin(pets, eq(users.id, pets.ownerId))
   *
   * // Select userId and petId
   * const usersIdsAndPetIds: { userId: number | null; petId: number | null; }[] = await db.select({
   *   userId: users.id,
   *   petId: pets.id,
   * })
   *   .from(users)
   *   .fullJoin(pets, eq(users.id, pets.ownerId))
   * ```
   */
  fullJoin = this.createJoin("full");
  /**
   * Executes a `cross join` operation by combining rows from two tables into a new table.
   *
   * Calling this method retrieves all rows from both main and joined tables, merging all rows from each table.
   *
   * See docs: {@link https://orm.drizzle.team/docs/joins#cross-join}
   *
   * @param table the table to join.
   *
   * @example
   *
   * ```ts
   * // Select all users, each user with every pet
   * const usersWithPets: { user: User; pets: Pet; }[] = await db.select()
   *   .from(users)
   *   .crossJoin(pets)
   *
   * // Select userId and petId
   * const usersIdsAndPetIds: { userId: number; petId: number; }[] = await db.select({
   *   userId: users.id,
   *   petId: pets.id,
   * })
   *   .from(users)
   *   .crossJoin(pets)
   * ```
   */
  crossJoin = this.createJoin("cross");
  createSetOperator(type, isAll) {
    return (rightSelection) => {
      const rightSelect = typeof rightSelection === "function" ? rightSelection(getSQLiteSetOperators()) : rightSelection;
      if (!haveSameKeys(this.getSelectedFields(), rightSelect.getSelectedFields())) {
        throw new Error(
          "Set operator error (union / intersect / except): selected fields are not the same or are in a different order"
        );
      }
      this.config.setOperators.push({ type, isAll, rightSelect });
      return this;
    };
  }
  /**
   * Adds `union` set operator to the query.
   *
   * Calling this method will combine the result sets of the `select` statements and remove any duplicate rows that appear across them.
   *
   * See docs: {@link https://orm.drizzle.team/docs/set-operations#union}
   *
   * @example
   *
   * ```ts
   * // Select all unique names from customers and users tables
   * await db.select({ name: users.name })
   *   .from(users)
   *   .union(
   *     db.select({ name: customers.name }).from(customers)
   *   );
   * // or
   * import { union } from 'drizzle-orm/sqlite-core'
   *
   * await union(
   *   db.select({ name: users.name }).from(users),
   *   db.select({ name: customers.name }).from(customers)
   * );
   * ```
   */
  union = this.createSetOperator("union", false);
  /**
   * Adds `union all` set operator to the query.
   *
   * Calling this method will combine the result-set of the `select` statements and keep all duplicate rows that appear across them.
   *
   * See docs: {@link https://orm.drizzle.team/docs/set-operations#union-all}
   *
   * @example
   *
   * ```ts
   * // Select all transaction ids from both online and in-store sales
   * await db.select({ transaction: onlineSales.transactionId })
   *   .from(onlineSales)
   *   .unionAll(
   *     db.select({ transaction: inStoreSales.transactionId }).from(inStoreSales)
   *   );
   * // or
   * import { unionAll } from 'drizzle-orm/sqlite-core'
   *
   * await unionAll(
   *   db.select({ transaction: onlineSales.transactionId }).from(onlineSales),
   *   db.select({ transaction: inStoreSales.transactionId }).from(inStoreSales)
   * );
   * ```
   */
  unionAll = this.createSetOperator("union", true);
  /**
   * Adds `intersect` set operator to the query.
   *
   * Calling this method will retain only the rows that are present in both result sets and eliminate duplicates.
   *
   * See docs: {@link https://orm.drizzle.team/docs/set-operations#intersect}
   *
   * @example
   *
   * ```ts
   * // Select course names that are offered in both departments A and B
   * await db.select({ courseName: depA.courseName })
   *   .from(depA)
   *   .intersect(
   *     db.select({ courseName: depB.courseName }).from(depB)
   *   );
   * // or
   * import { intersect } from 'drizzle-orm/sqlite-core'
   *
   * await intersect(
   *   db.select({ courseName: depA.courseName }).from(depA),
   *   db.select({ courseName: depB.courseName }).from(depB)
   * );
   * ```
   */
  intersect = this.createSetOperator("intersect", false);
  /**
   * Adds `except` set operator to the query.
   *
   * Calling this method will retrieve all unique rows from the left query, except for the rows that are present in the result set of the right query.
   *
   * See docs: {@link https://orm.drizzle.team/docs/set-operations#except}
   *
   * @example
   *
   * ```ts
   * // Select all courses offered in department A but not in department B
   * await db.select({ courseName: depA.courseName })
   *   .from(depA)
   *   .except(
   *     db.select({ courseName: depB.courseName }).from(depB)
   *   );
   * // or
   * import { except } from 'drizzle-orm/sqlite-core'
   *
   * await except(
   *   db.select({ courseName: depA.courseName }).from(depA),
   *   db.select({ courseName: depB.courseName }).from(depB)
   * );
   * ```
   */
  except = this.createSetOperator("except", false);
  /** @internal */
  addSetOperators(setOperators) {
    this.config.setOperators.push(...setOperators);
    return this;
  }
  /**
   * Adds a `where` clause to the query.
   *
   * Calling this method will select only those rows that fulfill a specified condition.
   *
   * See docs: {@link https://orm.drizzle.team/docs/select#filtering}
   *
   * @param where the `where` clause.
   *
   * @example
   * You can use conditional operators and `sql function` to filter the rows to be selected.
   *
   * ```ts
   * // Select all cars with green color
   * await db.select().from(cars).where(eq(cars.color, 'green'));
   * // or
   * await db.select().from(cars).where(sql`${cars.color} = 'green'`)
   * ```
   *
   * You can logically combine conditional operators with `and()` and `or()` operators:
   *
   * ```ts
   * // Select all BMW cars with a green color
   * await db.select().from(cars).where(and(eq(cars.color, 'green'), eq(cars.brand, 'BMW')));
   *
   * // Select all cars with the green or blue color
   * await db.select().from(cars).where(or(eq(cars.color, 'green'), eq(cars.color, 'blue')));
   * ```
   */
  where(where) {
    if (typeof where === "function") {
      where = where(
        new Proxy(
          this.config.fields,
          new SelectionProxyHandler({ sqlAliasedBehavior: "sql", sqlBehavior: "sql" })
        )
      );
    }
    this.config.where = where;
    return this;
  }
  /**
   * Adds a `having` clause to the query.
   *
   * Calling this method will select only those rows that fulfill a specified condition. It is typically used with aggregate functions to filter the aggregated data based on a specified condition.
   *
   * See docs: {@link https://orm.drizzle.team/docs/select#aggregations}
   *
   * @param having the `having` clause.
   *
   * @example
   *
   * ```ts
   * // Select all brands with more than one car
   * await db.select({
   * 	brand: cars.brand,
   * 	count: sql<number>`cast(count(${cars.id}) as int)`,
   * })
   *   .from(cars)
   *   .groupBy(cars.brand)
   *   .having(({ count }) => gt(count, 1));
   * ```
   */
  having(having) {
    if (typeof having === "function") {
      having = having(
        new Proxy(
          this.config.fields,
          new SelectionProxyHandler({ sqlAliasedBehavior: "sql", sqlBehavior: "sql" })
        )
      );
    }
    this.config.having = having;
    return this;
  }
  groupBy(...columns) {
    if (typeof columns[0] === "function") {
      const groupBy = columns[0](
        new Proxy(
          this.config.fields,
          new SelectionProxyHandler({ sqlAliasedBehavior: "alias", sqlBehavior: "sql" })
        )
      );
      this.config.groupBy = Array.isArray(groupBy) ? groupBy : [groupBy];
    } else {
      this.config.groupBy = columns;
    }
    return this;
  }
  orderBy(...columns) {
    if (typeof columns[0] === "function") {
      const orderBy = columns[0](
        new Proxy(
          this.config.fields,
          new SelectionProxyHandler({ sqlAliasedBehavior: "alias", sqlBehavior: "sql" })
        )
      );
      const orderByArray = Array.isArray(orderBy) ? orderBy : [orderBy];
      if (this.config.setOperators.length > 0) {
        this.config.setOperators.at(-1).orderBy = orderByArray;
      } else {
        this.config.orderBy = orderByArray;
      }
    } else {
      const orderByArray = columns;
      if (this.config.setOperators.length > 0) {
        this.config.setOperators.at(-1).orderBy = orderByArray;
      } else {
        this.config.orderBy = orderByArray;
      }
    }
    return this;
  }
  /**
   * Adds a `limit` clause to the query.
   *
   * Calling this method will set the maximum number of rows that will be returned by this query.
   *
   * See docs: {@link https://orm.drizzle.team/docs/select#limit--offset}
   *
   * @param limit the `limit` clause.
   *
   * @example
   *
   * ```ts
   * // Get the first 10 people from this query.
   * await db.select().from(people).limit(10);
   * ```
   */
  limit(limit) {
    if (this.config.setOperators.length > 0) {
      this.config.setOperators.at(-1).limit = limit;
    } else {
      this.config.limit = limit;
    }
    return this;
  }
  /**
   * Adds an `offset` clause to the query.
   *
   * Calling this method will skip a number of rows when returning results from this query.
   *
   * See docs: {@link https://orm.drizzle.team/docs/select#limit--offset}
   *
   * @param offset the `offset` clause.
   *
   * @example
   *
   * ```ts
   * // Get the 10th-20th people from this query.
   * await db.select().from(people).offset(10).limit(10);
   * ```
   */
  offset(offset) {
    if (this.config.setOperators.length > 0) {
      this.config.setOperators.at(-1).offset = offset;
    } else {
      this.config.offset = offset;
    }
    return this;
  }
  /** @internal */
  getSQL() {
    return this.dialect.buildSelectQuery(this.config);
  }
  toSQL() {
    const { typings: _typings, ...rest } = this.dialect.sqlToQuery(this.getSQL());
    return rest;
  }
  as(alias) {
    const usedTables = [];
    usedTables.push(...extractUsedTable(this.config.table));
    if (this.config.joins) {
      for (const it of this.config.joins) usedTables.push(...extractUsedTable(it.table));
    }
    return new Proxy(
      new Subquery(this.getSQL(), this.config.fields, alias, false, [...new Set(usedTables)]),
      new SelectionProxyHandler({ alias, sqlAliasedBehavior: "alias", sqlBehavior: "error" })
    );
  }
  /** @internal */
  getSelectedFields() {
    return new Proxy(
      this.config.fields,
      new SelectionProxyHandler({ alias: this.tableName, sqlAliasedBehavior: "alias", sqlBehavior: "error" })
    );
  }
  $dynamic() {
    return this;
  }
}
class SQLiteSelectBase extends SQLiteSelectQueryBuilderBase {
  static [entityKind] = "SQLiteSelect";
  /** @internal */
  _prepare(isOneTimeQuery = true) {
    if (!this.session) {
      throw new Error("Cannot execute a query on a query builder. Please use a database instance instead.");
    }
    const fieldsList = orderSelectedFields(this.config.fields);
    const query = this.session[isOneTimeQuery ? "prepareOneTimeQuery" : "prepareQuery"](
      this.dialect.sqlToQuery(this.getSQL()),
      fieldsList,
      "all",
      true,
      void 0,
      {
        type: "select",
        tables: [...this.usedTables]
      },
      this.cacheConfig
    );
    query.joinsNotNullableMap = this.joinsNotNullableMap;
    return query;
  }
  $withCache(config) {
    this.cacheConfig = config === void 0 ? { config: {}, enable: true, autoInvalidate: true } : config === false ? { enable: false } : { enable: true, autoInvalidate: true, ...config };
    return this;
  }
  prepare() {
    return this._prepare(false);
  }
  run = (placeholderValues) => {
    return this._prepare().run(placeholderValues);
  };
  all = (placeholderValues) => {
    return this._prepare().all(placeholderValues);
  };
  get = (placeholderValues) => {
    return this._prepare().get(placeholderValues);
  };
  values = (placeholderValues) => {
    return this._prepare().values(placeholderValues);
  };
  async execute() {
    return this.all();
  }
}
applyMixins(SQLiteSelectBase, [QueryPromise]);
function createSetOperator(type, isAll) {
  return (leftSelect, rightSelect, ...restSelects) => {
    const setOperators = [rightSelect, ...restSelects].map((select) => ({
      type,
      isAll,
      rightSelect: select
    }));
    for (const setOperator of setOperators) {
      if (!haveSameKeys(leftSelect.getSelectedFields(), setOperator.rightSelect.getSelectedFields())) {
        throw new Error(
          "Set operator error (union / intersect / except): selected fields are not the same or are in a different order"
        );
      }
    }
    return leftSelect.addSetOperators(setOperators);
  };
}
const getSQLiteSetOperators = () => ({
  union,
  unionAll,
  intersect,
  except
});
const union = createSetOperator("union", false);
const unionAll = createSetOperator("union", true);
const intersect = createSetOperator("intersect", false);
const except = createSetOperator("except", false);
class QueryBuilder {
  static [entityKind] = "SQLiteQueryBuilder";
  dialect;
  dialectConfig;
  constructor(dialect) {
    this.dialect = is(dialect, SQLiteDialect) ? dialect : void 0;
    this.dialectConfig = is(dialect, SQLiteDialect) ? void 0 : dialect;
  }
  $with = (alias, selection) => {
    const queryBuilder = this;
    const as = (qb) => {
      if (typeof qb === "function") {
        qb = qb(queryBuilder);
      }
      return new Proxy(
        new WithSubquery(
          qb.getSQL(),
          selection ?? ("getSelectedFields" in qb ? qb.getSelectedFields() ?? {} : {}),
          alias,
          true
        ),
        new SelectionProxyHandler({ alias, sqlAliasedBehavior: "alias", sqlBehavior: "error" })
      );
    };
    return { as };
  };
  with(...queries) {
    const self = this;
    function select(fields) {
      return new SQLiteSelectBuilder({
        fields: fields ?? void 0,
        session: void 0,
        dialect: self.getDialect(),
        withList: queries
      });
    }
    function selectDistinct(fields) {
      return new SQLiteSelectBuilder({
        fields: fields ?? void 0,
        session: void 0,
        dialect: self.getDialect(),
        withList: queries,
        distinct: true
      });
    }
    return { select, selectDistinct };
  }
  select(fields) {
    return new SQLiteSelectBuilder({ fields: fields ?? void 0, session: void 0, dialect: this.getDialect() });
  }
  selectDistinct(fields) {
    return new SQLiteSelectBuilder({
      fields: fields ?? void 0,
      session: void 0,
      dialect: this.getDialect(),
      distinct: true
    });
  }
  // Lazy load dialect to avoid circular dependency
  getDialect() {
    if (!this.dialect) {
      this.dialect = new SQLiteSyncDialect(this.dialectConfig);
    }
    return this.dialect;
  }
}
class SQLiteInsertBuilder {
  constructor(table, session, dialect, withList) {
    this.table = table;
    this.session = session;
    this.dialect = dialect;
    this.withList = withList;
  }
  static [entityKind] = "SQLiteInsertBuilder";
  values(values) {
    values = Array.isArray(values) ? values : [values];
    if (values.length === 0) {
      throw new Error("values() must be called with at least one value");
    }
    const mappedValues = values.map((entry) => {
      const result = {};
      const cols = this.table[Table.Symbol.Columns];
      for (const colKey of Object.keys(entry)) {
        const colValue = entry[colKey];
        result[colKey] = is(colValue, SQL) ? colValue : new Param(colValue, cols[colKey]);
      }
      return result;
    });
    return new SQLiteInsertBase(this.table, mappedValues, this.session, this.dialect, this.withList);
  }
  select(selectQuery) {
    const select = typeof selectQuery === "function" ? selectQuery(new QueryBuilder()) : selectQuery;
    if (!is(select, SQL) && !haveSameKeys(this.table[Columns], select._.selectedFields)) {
      throw new Error(
        "Insert select error: selected fields are not the same or are in a different order compared to the table definition"
      );
    }
    return new SQLiteInsertBase(this.table, select, this.session, this.dialect, this.withList, true);
  }
}
class SQLiteInsertBase extends QueryPromise {
  constructor(table, values, session, dialect, withList, select) {
    super();
    this.session = session;
    this.dialect = dialect;
    this.config = { table, values, withList, select };
  }
  static [entityKind] = "SQLiteInsert";
  /** @internal */
  config;
  returning(fields = this.config.table[SQLiteTable.Symbol.Columns]) {
    this.config.returning = orderSelectedFields(fields);
    return this;
  }
  /**
   * Adds an `on conflict do nothing` clause to the query.
   *
   * Calling this method simply avoids inserting a row as its alternative action.
   *
   * See docs: {@link https://orm.drizzle.team/docs/insert#on-conflict-do-nothing}
   *
   * @param config The `target` and `where` clauses.
   *
   * @example
   * ```ts
   * // Insert one row and cancel the insert if there's a conflict
   * await db.insert(cars)
   *   .values({ id: 1, brand: 'BMW' })
   *   .onConflictDoNothing();
   *
   * // Explicitly specify conflict target
   * await db.insert(cars)
   *   .values({ id: 1, brand: 'BMW' })
   *   .onConflictDoNothing({ target: cars.id });
   * ```
   */
  onConflictDoNothing(config = {}) {
    if (!this.config.onConflict) this.config.onConflict = [];
    if (config.target === void 0) {
      this.config.onConflict.push(sql` on conflict do nothing`);
    } else {
      const targetSql = Array.isArray(config.target) ? sql`${config.target}` : sql`${[config.target]}`;
      const whereSql = config.where ? sql` where ${config.where}` : sql``;
      this.config.onConflict.push(sql` on conflict ${targetSql} do nothing${whereSql}`);
    }
    return this;
  }
  /**
   * Adds an `on conflict do update` clause to the query.
   *
   * Calling this method will update the existing row that conflicts with the row proposed for insertion as its alternative action.
   *
   * See docs: {@link https://orm.drizzle.team/docs/insert#upserts-and-conflicts}
   *
   * @param config The `target`, `set` and `where` clauses.
   *
   * @example
   * ```ts
   * // Update the row if there's a conflict
   * await db.insert(cars)
   *   .values({ id: 1, brand: 'BMW' })
   *   .onConflictDoUpdate({
   *     target: cars.id,
   *     set: { brand: 'Porsche' }
   *   });
   *
   * // Upsert with 'where' clause
   * await db.insert(cars)
   *   .values({ id: 1, brand: 'BMW' })
   *   .onConflictDoUpdate({
   *     target: cars.id,
   *     set: { brand: 'newBMW' },
   *     where: sql`${cars.createdAt} > '2023-01-01'::date`,
   *   });
   * ```
   */
  onConflictDoUpdate(config) {
    if (config.where && (config.targetWhere || config.setWhere)) {
      throw new Error(
        'You cannot use both "where" and "targetWhere"/"setWhere" at the same time - "where" is deprecated, use "targetWhere" or "setWhere" instead.'
      );
    }
    if (!this.config.onConflict) this.config.onConflict = [];
    const whereSql = config.where ? sql` where ${config.where}` : void 0;
    const targetWhereSql = config.targetWhere ? sql` where ${config.targetWhere}` : void 0;
    const setWhereSql = config.setWhere ? sql` where ${config.setWhere}` : void 0;
    const targetSql = Array.isArray(config.target) ? sql`${config.target}` : sql`${[config.target]}`;
    const setSql = this.dialect.buildUpdateSet(this.config.table, mapUpdateSet(this.config.table, config.set));
    this.config.onConflict.push(
      sql` on conflict ${targetSql}${targetWhereSql} do update set ${setSql}${whereSql}${setWhereSql}`
    );
    return this;
  }
  /** @internal */
  getSQL() {
    return this.dialect.buildInsertQuery(this.config);
  }
  toSQL() {
    const { typings: _typings, ...rest } = this.dialect.sqlToQuery(this.getSQL());
    return rest;
  }
  /** @internal */
  _prepare(isOneTimeQuery = true) {
    return this.session[isOneTimeQuery ? "prepareOneTimeQuery" : "prepareQuery"](
      this.dialect.sqlToQuery(this.getSQL()),
      this.config.returning,
      this.config.returning ? "all" : "run",
      true,
      void 0,
      {
        type: "insert",
        tables: extractUsedTable(this.config.table)
      }
    );
  }
  prepare() {
    return this._prepare(false);
  }
  run = (placeholderValues) => {
    return this._prepare().run(placeholderValues);
  };
  all = (placeholderValues) => {
    return this._prepare().all(placeholderValues);
  };
  get = (placeholderValues) => {
    return this._prepare().get(placeholderValues);
  };
  values = (placeholderValues) => {
    return this._prepare().values(placeholderValues);
  };
  async execute() {
    return this.config.returning ? this.all() : this.run();
  }
  $dynamic() {
    return this;
  }
}
class SQLiteUpdateBuilder {
  constructor(table, session, dialect, withList) {
    this.table = table;
    this.session = session;
    this.dialect = dialect;
    this.withList = withList;
  }
  static [entityKind] = "SQLiteUpdateBuilder";
  set(values) {
    return new SQLiteUpdateBase(
      this.table,
      mapUpdateSet(this.table, values),
      this.session,
      this.dialect,
      this.withList
    );
  }
}
class SQLiteUpdateBase extends QueryPromise {
  constructor(table, set, session, dialect, withList) {
    super();
    this.session = session;
    this.dialect = dialect;
    this.config = { set, table, withList, joins: [] };
  }
  static [entityKind] = "SQLiteUpdate";
  /** @internal */
  config;
  from(source) {
    this.config.from = source;
    return this;
  }
  createJoin(joinType) {
    return (table, on) => {
      const tableName = getTableLikeName(table);
      if (typeof tableName === "string" && this.config.joins.some((join) => join.alias === tableName)) {
        throw new Error(`Alias "${tableName}" is already used in this query`);
      }
      if (typeof on === "function") {
        const from = this.config.from ? is(table, SQLiteTable) ? table[Table.Symbol.Columns] : is(table, Subquery) ? table._.selectedFields : is(table, SQLiteViewBase) ? table[ViewBaseConfig].selectedFields : void 0 : void 0;
        on = on(
          new Proxy(
            this.config.table[Table.Symbol.Columns],
            new SelectionProxyHandler({ sqlAliasedBehavior: "sql", sqlBehavior: "sql" })
          ),
          from && new Proxy(
            from,
            new SelectionProxyHandler({ sqlAliasedBehavior: "sql", sqlBehavior: "sql" })
          )
        );
      }
      this.config.joins.push({ on, table, joinType, alias: tableName });
      return this;
    };
  }
  leftJoin = this.createJoin("left");
  rightJoin = this.createJoin("right");
  innerJoin = this.createJoin("inner");
  fullJoin = this.createJoin("full");
  /**
   * Adds a 'where' clause to the query.
   *
   * Calling this method will update only those rows that fulfill a specified condition.
   *
   * See docs: {@link https://orm.drizzle.team/docs/update}
   *
   * @param where the 'where' clause.
   *
   * @example
   * You can use conditional operators and `sql function` to filter the rows to be updated.
   *
   * ```ts
   * // Update all cars with green color
   * db.update(cars).set({ color: 'red' })
   *   .where(eq(cars.color, 'green'));
   * // or
   * db.update(cars).set({ color: 'red' })
   *   .where(sql`${cars.color} = 'green'`)
   * ```
   *
   * You can logically combine conditional operators with `and()` and `or()` operators:
   *
   * ```ts
   * // Update all BMW cars with a green color
   * db.update(cars).set({ color: 'red' })
   *   .where(and(eq(cars.color, 'green'), eq(cars.brand, 'BMW')));
   *
   * // Update all cars with the green or blue color
   * db.update(cars).set({ color: 'red' })
   *   .where(or(eq(cars.color, 'green'), eq(cars.color, 'blue')));
   * ```
   */
  where(where) {
    this.config.where = where;
    return this;
  }
  orderBy(...columns) {
    if (typeof columns[0] === "function") {
      const orderBy = columns[0](
        new Proxy(
          this.config.table[Table.Symbol.Columns],
          new SelectionProxyHandler({ sqlAliasedBehavior: "alias", sqlBehavior: "sql" })
        )
      );
      const orderByArray = Array.isArray(orderBy) ? orderBy : [orderBy];
      this.config.orderBy = orderByArray;
    } else {
      const orderByArray = columns;
      this.config.orderBy = orderByArray;
    }
    return this;
  }
  limit(limit) {
    this.config.limit = limit;
    return this;
  }
  returning(fields = this.config.table[SQLiteTable.Symbol.Columns]) {
    this.config.returning = orderSelectedFields(fields);
    return this;
  }
  /** @internal */
  getSQL() {
    return this.dialect.buildUpdateQuery(this.config);
  }
  toSQL() {
    const { typings: _typings, ...rest } = this.dialect.sqlToQuery(this.getSQL());
    return rest;
  }
  /** @internal */
  _prepare(isOneTimeQuery = true) {
    return this.session[isOneTimeQuery ? "prepareOneTimeQuery" : "prepareQuery"](
      this.dialect.sqlToQuery(this.getSQL()),
      this.config.returning,
      this.config.returning ? "all" : "run",
      true,
      void 0,
      {
        type: "insert",
        tables: extractUsedTable(this.config.table)
      }
    );
  }
  prepare() {
    return this._prepare(false);
  }
  run = (placeholderValues) => {
    return this._prepare().run(placeholderValues);
  };
  all = (placeholderValues) => {
    return this._prepare().all(placeholderValues);
  };
  get = (placeholderValues) => {
    return this._prepare().get(placeholderValues);
  };
  values = (placeholderValues) => {
    return this._prepare().values(placeholderValues);
  };
  async execute() {
    return this.config.returning ? this.all() : this.run();
  }
  $dynamic() {
    return this;
  }
}
class SQLiteCountBuilder extends SQL {
  constructor(params) {
    super(SQLiteCountBuilder.buildEmbeddedCount(params.source, params.filters).queryChunks);
    this.params = params;
    this.session = params.session;
    this.sql = SQLiteCountBuilder.buildCount(
      params.source,
      params.filters
    );
  }
  sql;
  static [entityKind] = "SQLiteCountBuilderAsync";
  [Symbol.toStringTag] = "SQLiteCountBuilderAsync";
  session;
  static buildEmbeddedCount(source, filters) {
    return sql`(select count(*) from ${source}${sql.raw(" where ").if(filters)}${filters})`;
  }
  static buildCount(source, filters) {
    return sql`select count(*) from ${source}${sql.raw(" where ").if(filters)}${filters}`;
  }
  then(onfulfilled, onrejected) {
    return Promise.resolve(this.session.count(this.sql)).then(
      onfulfilled,
      onrejected
    );
  }
  catch(onRejected) {
    return this.then(void 0, onRejected);
  }
  finally(onFinally) {
    return this.then(
      (value) => {
        onFinally?.();
        return value;
      },
      (reason) => {
        onFinally?.();
        throw reason;
      }
    );
  }
}
class RelationalQueryBuilder {
  constructor(mode, fullSchema, schema2, tableNamesMap, table, tableConfig, dialect, session) {
    this.mode = mode;
    this.fullSchema = fullSchema;
    this.schema = schema2;
    this.tableNamesMap = tableNamesMap;
    this.table = table;
    this.tableConfig = tableConfig;
    this.dialect = dialect;
    this.session = session;
  }
  static [entityKind] = "SQLiteAsyncRelationalQueryBuilder";
  findMany(config) {
    return this.mode === "sync" ? new SQLiteSyncRelationalQuery(
      this.fullSchema,
      this.schema,
      this.tableNamesMap,
      this.table,
      this.tableConfig,
      this.dialect,
      this.session,
      config ? config : {},
      "many"
    ) : new SQLiteRelationalQuery(
      this.fullSchema,
      this.schema,
      this.tableNamesMap,
      this.table,
      this.tableConfig,
      this.dialect,
      this.session,
      config ? config : {},
      "many"
    );
  }
  findFirst(config) {
    return this.mode === "sync" ? new SQLiteSyncRelationalQuery(
      this.fullSchema,
      this.schema,
      this.tableNamesMap,
      this.table,
      this.tableConfig,
      this.dialect,
      this.session,
      config ? { ...config, limit: 1 } : { limit: 1 },
      "first"
    ) : new SQLiteRelationalQuery(
      this.fullSchema,
      this.schema,
      this.tableNamesMap,
      this.table,
      this.tableConfig,
      this.dialect,
      this.session,
      config ? { ...config, limit: 1 } : { limit: 1 },
      "first"
    );
  }
}
class SQLiteRelationalQuery extends QueryPromise {
  constructor(fullSchema, schema2, tableNamesMap, table, tableConfig, dialect, session, config, mode) {
    super();
    this.fullSchema = fullSchema;
    this.schema = schema2;
    this.tableNamesMap = tableNamesMap;
    this.table = table;
    this.tableConfig = tableConfig;
    this.dialect = dialect;
    this.session = session;
    this.config = config;
    this.mode = mode;
  }
  static [entityKind] = "SQLiteAsyncRelationalQuery";
  /** @internal */
  mode;
  /** @internal */
  getSQL() {
    return this.dialect.buildRelationalQuery({
      fullSchema: this.fullSchema,
      schema: this.schema,
      tableNamesMap: this.tableNamesMap,
      table: this.table,
      tableConfig: this.tableConfig,
      queryConfig: this.config,
      tableAlias: this.tableConfig.tsName
    }).sql;
  }
  /** @internal */
  _prepare(isOneTimeQuery = false) {
    const { query, builtQuery } = this._toSQL();
    return this.session[isOneTimeQuery ? "prepareOneTimeQuery" : "prepareQuery"](
      builtQuery,
      void 0,
      this.mode === "first" ? "get" : "all",
      true,
      (rawRows, mapColumnValue) => {
        const rows = rawRows.map(
          (row) => mapRelationalRow(this.schema, this.tableConfig, row, query.selection, mapColumnValue)
        );
        if (this.mode === "first") {
          return rows[0];
        }
        return rows;
      }
    );
  }
  prepare() {
    return this._prepare(false);
  }
  _toSQL() {
    const query = this.dialect.buildRelationalQuery({
      fullSchema: this.fullSchema,
      schema: this.schema,
      tableNamesMap: this.tableNamesMap,
      table: this.table,
      tableConfig: this.tableConfig,
      queryConfig: this.config,
      tableAlias: this.tableConfig.tsName
    });
    const builtQuery = this.dialect.sqlToQuery(query.sql);
    return { query, builtQuery };
  }
  toSQL() {
    return this._toSQL().builtQuery;
  }
  /** @internal */
  executeRaw() {
    if (this.mode === "first") {
      return this._prepare(false).get();
    }
    return this._prepare(false).all();
  }
  async execute() {
    return this.executeRaw();
  }
}
class SQLiteSyncRelationalQuery extends SQLiteRelationalQuery {
  static [entityKind] = "SQLiteSyncRelationalQuery";
  sync() {
    return this.executeRaw();
  }
}
class SQLiteRaw extends QueryPromise {
  constructor(execute, getSQL, action, dialect, mapBatchResult) {
    super();
    this.execute = execute;
    this.getSQL = getSQL;
    this.dialect = dialect;
    this.mapBatchResult = mapBatchResult;
    this.config = { action };
  }
  static [entityKind] = "SQLiteRaw";
  /** @internal */
  config;
  getQuery() {
    return { ...this.dialect.sqlToQuery(this.getSQL()), method: this.config.action };
  }
  mapResult(result, isFromBatch) {
    return isFromBatch ? this.mapBatchResult(result) : result;
  }
  _prepare() {
    return this;
  }
  /** @internal */
  isResponseInArrayMode() {
    return false;
  }
}
class BaseSQLiteDatabase {
  constructor(resultKind, dialect, session, schema2) {
    this.resultKind = resultKind;
    this.dialect = dialect;
    this.session = session;
    this._ = schema2 ? {
      schema: schema2.schema,
      fullSchema: schema2.fullSchema,
      tableNamesMap: schema2.tableNamesMap
    } : {
      schema: void 0,
      fullSchema: {},
      tableNamesMap: {}
    };
    this.query = {};
    const query = this.query;
    if (this._.schema) {
      for (const [tableName, columns] of Object.entries(this._.schema)) {
        query[tableName] = new RelationalQueryBuilder(
          resultKind,
          schema2.fullSchema,
          this._.schema,
          this._.tableNamesMap,
          schema2.fullSchema[tableName],
          columns,
          dialect,
          session
        );
      }
    }
    this.$cache = { invalidate: async (_params) => {
    } };
  }
  static [entityKind] = "BaseSQLiteDatabase";
  query;
  /**
   * Creates a subquery that defines a temporary named result set as a CTE.
   *
   * It is useful for breaking down complex queries into simpler parts and for reusing the result set in subsequent parts of the query.
   *
   * See docs: {@link https://orm.drizzle.team/docs/select#with-clause}
   *
   * @param alias The alias for the subquery.
   *
   * Failure to provide an alias will result in a DrizzleTypeError, preventing the subquery from being referenced in other queries.
   *
   * @example
   *
   * ```ts
   * // Create a subquery with alias 'sq' and use it in the select query
   * const sq = db.$with('sq').as(db.select().from(users).where(eq(users.id, 42)));
   *
   * const result = await db.with(sq).select().from(sq);
   * ```
   *
   * To select arbitrary SQL values as fields in a CTE and reference them in other CTEs or in the main query, you need to add aliases to them:
   *
   * ```ts
   * // Select an arbitrary SQL value as a field in a CTE and reference it in the main query
   * const sq = db.$with('sq').as(db.select({
   *   name: sql<string>`upper(${users.name})`.as('name'),
   * })
   * .from(users));
   *
   * const result = await db.with(sq).select({ name: sq.name }).from(sq);
   * ```
   */
  $with = (alias, selection) => {
    const self = this;
    const as = (qb) => {
      if (typeof qb === "function") {
        qb = qb(new QueryBuilder(self.dialect));
      }
      return new Proxy(
        new WithSubquery(
          qb.getSQL(),
          selection ?? ("getSelectedFields" in qb ? qb.getSelectedFields() ?? {} : {}),
          alias,
          true
        ),
        new SelectionProxyHandler({ alias, sqlAliasedBehavior: "alias", sqlBehavior: "error" })
      );
    };
    return { as };
  };
  $count(source, filters) {
    return new SQLiteCountBuilder({ source, filters, session: this.session });
  }
  /**
   * Incorporates a previously defined CTE (using `$with`) into the main query.
   *
   * This method allows the main query to reference a temporary named result set.
   *
   * See docs: {@link https://orm.drizzle.team/docs/select#with-clause}
   *
   * @param queries The CTEs to incorporate into the main query.
   *
   * @example
   *
   * ```ts
   * // Define a subquery 'sq' as a CTE using $with
   * const sq = db.$with('sq').as(db.select().from(users).where(eq(users.id, 42)));
   *
   * // Incorporate the CTE 'sq' into the main query and select from it
   * const result = await db.with(sq).select().from(sq);
   * ```
   */
  with(...queries) {
    const self = this;
    function select(fields) {
      return new SQLiteSelectBuilder({
        fields: fields ?? void 0,
        session: self.session,
        dialect: self.dialect,
        withList: queries
      });
    }
    function selectDistinct(fields) {
      return new SQLiteSelectBuilder({
        fields: fields ?? void 0,
        session: self.session,
        dialect: self.dialect,
        withList: queries,
        distinct: true
      });
    }
    function update(table) {
      return new SQLiteUpdateBuilder(table, self.session, self.dialect, queries);
    }
    function insert(into) {
      return new SQLiteInsertBuilder(into, self.session, self.dialect, queries);
    }
    function delete_(from) {
      return new SQLiteDeleteBase(from, self.session, self.dialect, queries);
    }
    return { select, selectDistinct, update, insert, delete: delete_ };
  }
  select(fields) {
    return new SQLiteSelectBuilder({ fields: fields ?? void 0, session: this.session, dialect: this.dialect });
  }
  selectDistinct(fields) {
    return new SQLiteSelectBuilder({
      fields: fields ?? void 0,
      session: this.session,
      dialect: this.dialect,
      distinct: true
    });
  }
  /**
   * Creates an update query.
   *
   * Calling this method without `.where()` clause will update all rows in a table. The `.where()` clause specifies which rows should be updated.
   *
   * Use `.set()` method to specify which values to update.
   *
   * See docs: {@link https://orm.drizzle.team/docs/update}
   *
   * @param table The table to update.
   *
   * @example
   *
   * ```ts
   * // Update all rows in the 'cars' table
   * await db.update(cars).set({ color: 'red' });
   *
   * // Update rows with filters and conditions
   * await db.update(cars).set({ color: 'red' }).where(eq(cars.brand, 'BMW'));
   *
   * // Update with returning clause
   * const updatedCar: Car[] = await db.update(cars)
   *   .set({ color: 'red' })
   *   .where(eq(cars.id, 1))
   *   .returning();
   * ```
   */
  update(table) {
    return new SQLiteUpdateBuilder(table, this.session, this.dialect);
  }
  $cache;
  /**
   * Creates an insert query.
   *
   * Calling this method will create new rows in a table. Use `.values()` method to specify which values to insert.
   *
   * See docs: {@link https://orm.drizzle.team/docs/insert}
   *
   * @param table The table to insert into.
   *
   * @example
   *
   * ```ts
   * // Insert one row
   * await db.insert(cars).values({ brand: 'BMW' });
   *
   * // Insert multiple rows
   * await db.insert(cars).values([{ brand: 'BMW' }, { brand: 'Porsche' }]);
   *
   * // Insert with returning clause
   * const insertedCar: Car[] = await db.insert(cars)
   *   .values({ brand: 'BMW' })
   *   .returning();
   * ```
   */
  insert(into) {
    return new SQLiteInsertBuilder(into, this.session, this.dialect);
  }
  /**
   * Creates a delete query.
   *
   * Calling this method without `.where()` clause will delete all rows in a table. The `.where()` clause specifies which rows should be deleted.
   *
   * See docs: {@link https://orm.drizzle.team/docs/delete}
   *
   * @param table The table to delete from.
   *
   * @example
   *
   * ```ts
   * // Delete all rows in the 'cars' table
   * await db.delete(cars);
   *
   * // Delete rows with filters and conditions
   * await db.delete(cars).where(eq(cars.color, 'green'));
   *
   * // Delete with returning clause
   * const deletedCar: Car[] = await db.delete(cars)
   *   .where(eq(cars.id, 1))
   *   .returning();
   * ```
   */
  delete(from) {
    return new SQLiteDeleteBase(from, this.session, this.dialect);
  }
  run(query) {
    const sequel = typeof query === "string" ? sql.raw(query) : query.getSQL();
    if (this.resultKind === "async") {
      return new SQLiteRaw(
        async () => this.session.run(sequel),
        () => sequel,
        "run",
        this.dialect,
        this.session.extractRawRunValueFromBatchResult.bind(this.session)
      );
    }
    return this.session.run(sequel);
  }
  all(query) {
    const sequel = typeof query === "string" ? sql.raw(query) : query.getSQL();
    if (this.resultKind === "async") {
      return new SQLiteRaw(
        async () => this.session.all(sequel),
        () => sequel,
        "all",
        this.dialect,
        this.session.extractRawAllValueFromBatchResult.bind(this.session)
      );
    }
    return this.session.all(sequel);
  }
  get(query) {
    const sequel = typeof query === "string" ? sql.raw(query) : query.getSQL();
    if (this.resultKind === "async") {
      return new SQLiteRaw(
        async () => this.session.get(sequel),
        () => sequel,
        "get",
        this.dialect,
        this.session.extractRawGetValueFromBatchResult.bind(this.session)
      );
    }
    return this.session.get(sequel);
  }
  values(query) {
    const sequel = typeof query === "string" ? sql.raw(query) : query.getSQL();
    if (this.resultKind === "async") {
      return new SQLiteRaw(
        async () => this.session.values(sequel),
        () => sequel,
        "values",
        this.dialect,
        this.session.extractRawValuesValueFromBatchResult.bind(this.session)
      );
    }
    return this.session.values(sequel);
  }
  transaction(transaction, config) {
    return this.session.transaction(transaction, config);
  }
}
class Cache {
  static [entityKind] = "Cache";
}
class NoopCache extends Cache {
  strategy() {
    return "all";
  }
  static [entityKind] = "NoopCache";
  async get(_key) {
    return void 0;
  }
  async put(_hashedQuery, _response, _tables, _config) {
  }
  async onMutate(_params) {
  }
}
async function hashQuery(sql2, params) {
  const dataToHash = `${sql2}-${JSON.stringify(params)}`;
  const encoder = new TextEncoder();
  const data = encoder.encode(dataToHash);
  const hashBuffer = await crypto.subtle.digest("SHA-256", data);
  const hashArray = [...new Uint8Array(hashBuffer)];
  const hashHex = hashArray.map((b) => b.toString(16).padStart(2, "0")).join("");
  return hashHex;
}
class ExecuteResultSync extends QueryPromise {
  constructor(resultCb) {
    super();
    this.resultCb = resultCb;
  }
  static [entityKind] = "ExecuteResultSync";
  async execute() {
    return this.resultCb();
  }
  sync() {
    return this.resultCb();
  }
}
class SQLitePreparedQuery {
  constructor(mode, executeMethod, query, cache, queryMetadata, cacheConfig) {
    this.mode = mode;
    this.executeMethod = executeMethod;
    this.query = query;
    this.cache = cache;
    this.queryMetadata = queryMetadata;
    this.cacheConfig = cacheConfig;
    if (cache && cache.strategy() === "all" && cacheConfig === void 0) {
      this.cacheConfig = { enable: true, autoInvalidate: true };
    }
    if (!this.cacheConfig?.enable) {
      this.cacheConfig = void 0;
    }
  }
  static [entityKind] = "PreparedQuery";
  /** @internal */
  joinsNotNullableMap;
  /** @internal */
  async queryWithCache(queryString, params, query) {
    if (this.cache === void 0 || is(this.cache, NoopCache) || this.queryMetadata === void 0) {
      try {
        return await query();
      } catch (e) {
        throw new DrizzleQueryError(queryString, params, e);
      }
    }
    if (this.cacheConfig && !this.cacheConfig.enable) {
      try {
        return await query();
      } catch (e) {
        throw new DrizzleQueryError(queryString, params, e);
      }
    }
    if ((this.queryMetadata.type === "insert" || this.queryMetadata.type === "update" || this.queryMetadata.type === "delete") && this.queryMetadata.tables.length > 0) {
      try {
        const [res] = await Promise.all([
          query(),
          this.cache.onMutate({ tables: this.queryMetadata.tables })
        ]);
        return res;
      } catch (e) {
        throw new DrizzleQueryError(queryString, params, e);
      }
    }
    if (!this.cacheConfig) {
      try {
        return await query();
      } catch (e) {
        throw new DrizzleQueryError(queryString, params, e);
      }
    }
    if (this.queryMetadata.type === "select") {
      const fromCache = await this.cache.get(
        this.cacheConfig.tag ?? await hashQuery(queryString, params),
        this.queryMetadata.tables,
        this.cacheConfig.tag !== void 0,
        this.cacheConfig.autoInvalidate
      );
      if (fromCache === void 0) {
        let result;
        try {
          result = await query();
        } catch (e) {
          throw new DrizzleQueryError(queryString, params, e);
        }
        await this.cache.put(
          this.cacheConfig.tag ?? await hashQuery(queryString, params),
          result,
          // make sure we send tables that were used in a query only if user wants to invalidate it on each write
          this.cacheConfig.autoInvalidate ? this.queryMetadata.tables : [],
          this.cacheConfig.tag !== void 0,
          this.cacheConfig.config
        );
        return result;
      }
      return fromCache;
    }
    try {
      return await query();
    } catch (e) {
      throw new DrizzleQueryError(queryString, params, e);
    }
  }
  getQuery() {
    return this.query;
  }
  mapRunResult(result, _isFromBatch) {
    return result;
  }
  mapAllResult(_result, _isFromBatch) {
    throw new Error("Not implemented");
  }
  mapGetResult(_result, _isFromBatch) {
    throw new Error("Not implemented");
  }
  execute(placeholderValues) {
    if (this.mode === "async") {
      return this[this.executeMethod](placeholderValues);
    }
    return new ExecuteResultSync(() => this[this.executeMethod](placeholderValues));
  }
  mapResult(response, isFromBatch) {
    switch (this.executeMethod) {
      case "run": {
        return this.mapRunResult(response, isFromBatch);
      }
      case "all": {
        return this.mapAllResult(response, isFromBatch);
      }
      case "get": {
        return this.mapGetResult(response, isFromBatch);
      }
    }
  }
}
class SQLiteSession {
  constructor(dialect) {
    this.dialect = dialect;
  }
  static [entityKind] = "SQLiteSession";
  prepareOneTimeQuery(query, fields, executeMethod, isResponseInArrayMode, customResultMapper, queryMetadata, cacheConfig) {
    return this.prepareQuery(
      query,
      fields,
      executeMethod,
      isResponseInArrayMode,
      customResultMapper,
      queryMetadata,
      cacheConfig
    );
  }
  run(query) {
    const staticQuery = this.dialect.sqlToQuery(query);
    try {
      return this.prepareOneTimeQuery(staticQuery, void 0, "run", false).run();
    } catch (err) {
      throw new DrizzleError({ cause: err, message: `Failed to run the query '${staticQuery.sql}'` });
    }
  }
  /** @internal */
  extractRawRunValueFromBatchResult(result) {
    return result;
  }
  all(query) {
    return this.prepareOneTimeQuery(this.dialect.sqlToQuery(query), void 0, "run", false).all();
  }
  /** @internal */
  extractRawAllValueFromBatchResult(_result) {
    throw new Error("Not implemented");
  }
  get(query) {
    return this.prepareOneTimeQuery(this.dialect.sqlToQuery(query), void 0, "run", false).get();
  }
  /** @internal */
  extractRawGetValueFromBatchResult(_result) {
    throw new Error("Not implemented");
  }
  values(query) {
    return this.prepareOneTimeQuery(this.dialect.sqlToQuery(query), void 0, "run", false).values();
  }
  async count(sql2) {
    const result = await this.values(sql2);
    return result[0][0];
  }
  /** @internal */
  extractRawValuesValueFromBatchResult(_result) {
    throw new Error("Not implemented");
  }
}
class SQLiteTransaction extends BaseSQLiteDatabase {
  constructor(resultType, dialect, session, schema2, nestedIndex = 0) {
    super(resultType, dialect, session, schema2);
    this.schema = schema2;
    this.nestedIndex = nestedIndex;
  }
  static [entityKind] = "SQLiteTransaction";
  rollback() {
    throw new TransactionRollbackError();
  }
}
const DIRECTIONS = ["minimize", "maximize"];
const OPTIMIZATION_STATUSES = [
  "draft",
  "baselining",
  "running",
  "stopping",
  "completed",
  "stopped",
  "failed"
];
const AGENT_KINDS = ["claude-code", "codex", "mock"];
const AGENT_STATUSES = [
  "pending",
  "launching",
  "running",
  "paused",
  "stopped",
  "done",
  "failed"
];
const VERDICTS = ["pending", "verified", "rejected", "failed_confirm", "blocked"];
const BENCH_PHASES = ["bench", "confirm"];
const SPEND_CATEGORIES = [
  "agent_runner",
  "experiment",
  "bench_runner",
  "storage_egress",
  "tokens"
];
const inList = (col, values) => sql`${col} in (${sql.raw(values.map((v) => `'${v}'`).join(", "))})`;
const optimizations = sqliteTable(
  "optimizations",
  {
    id: text("id").primaryKey(),
    name: text("name"),
    repo: text("repo").notNull(),
    baseRef: text("base_ref").notNull(),
    metric: text("metric").notNull(),
    direction: text("direction", { enum: DIRECTIONS }).notNull(),
    /** Minimum relative gain, e.g. 0.03 = 3%. */
    minGain: real("min_gain").notNull(),
    budgetUsd: real("budget_usd").notNull(),
    timeLimitS: integer("time_limit_s").notNull(),
    status: text("status", { enum: OPTIMIZATION_STATUSES }).notNull().default("draft"),
    createdAt: integer("created_at").notNull(),
    startedAt: integer("started_at"),
    endedAt: integer("ended_at"),
    /** Snapshot of ratchet.yaml at creation time. */
    configYaml: text("config_yaml").notNull()
  },
  (t) => [
    check("optimizations_direction_check", inList(t.direction, DIRECTIONS)),
    check("optimizations_status_check", inList(t.status, OPTIMIZATION_STATUSES))
  ]
);
const baselines = sqliteTable("baselines", {
  optimizationId: text("optimization_id").primaryKey().references(() => optimizations.id),
  samples: text("samples", { mode: "json" }).$type().notNull(),
  /** The baseline headline value (the ratchet starts here). */
  value: real("value").notNull(),
  noiseFloor: real("noise_floor").notNull(),
  stats: text("stats", { mode: "json" }).$type().notNull(),
  recordedAt: integer("recorded_at").notNull()
});
const agents = sqliteTable(
  "agents",
  {
    id: text("id").primaryKey(),
    optimizationId: text("optimization_id").notNull().references(() => optimizations.id),
    kind: text("kind", { enum: AGENT_KINDS }).notNull(),
    branch: text("branch").notNull(),
    runnerInstanceId: text("runner_instance_id"),
    status: text("status", { enum: AGENT_STATUSES }).notNull().default("pending"),
    createdAt: integer("created_at").notNull()
  },
  (t) => [
    index("agents_optimization_idx").on(t.optimizationId),
    check("agents_kind_check", inList(t.kind, AGENT_KINDS)),
    check("agents_status_check", inList(t.status, AGENT_STATUSES))
  ]
);
const candidates = sqliteTable(
  "candidates",
  {
    id: text("id").primaryKey(),
    optimizationId: text("optimization_id").notNull().references(() => optimizations.id),
    agentId: text("agent_id").notNull().references(() => agents.id),
    sha: text("sha").notNull(),
    message: text("message").notNull().default(""),
    submittedAt: integer("submitted_at").notNull(),
    verdict: text("verdict", { enum: VERDICTS }).notNull().default("pending"),
    /** When the verdict was set (null while pending). */
    verdictAt: integer("verdict_at"),
    reasons: text("reasons", { mode: "json" }).$type().notNull().default(sql`'[]'`)
  },
  (t) => [
    index("candidates_optimization_idx").on(t.optimizationId, t.submittedAt),
    index("candidates_agent_idx").on(t.agentId),
    uniqueIndex("candidates_agent_sha_unique").on(t.agentId, t.sha),
    check("candidates_verdict_check", inList(t.verdict, VERDICTS))
  ]
);
const benchResults = sqliteTable(
  "bench_results",
  {
    id: integer("id").primaryKey({ autoIncrement: true }),
    candidateId: text("candidate_id").notNull().references(() => candidates.id),
    phase: text("phase", { enum: BENCH_PHASES }).notNull(),
    samples: text("samples", { mode: "json" }).$type().notNull(),
    stats: text("stats", { mode: "json" }).$type().notNull(),
    guardrails: text("guardrails", { mode: "json" }).$type().notNull().default(sql`'[]'`),
    startedAt: integer("started_at").notNull(),
    finishedAt: integer("finished_at").notNull()
  },
  (t) => [
    index("bench_results_candidate_idx").on(t.candidateId),
    check("bench_results_phase_check", inList(t.phase, BENCH_PHASES))
  ]
);
const ratchetMoves = sqliteTable(
  "ratchet_moves",
  {
    id: integer("id").primaryKey({ autoIncrement: true }),
    optimizationId: text("optimization_id").notNull().references(() => optimizations.id),
    candidateId: text("candidate_id").notNull().references(() => candidates.id),
    fromValue: real("from_value").notNull(),
    toValue: real("to_value").notNull(),
    at: integer("at").notNull()
  },
  (t) => [
    index("ratchet_moves_optimization_idx").on(t.optimizationId, t.id),
    uniqueIndex("ratchet_moves_candidate_unique").on(t.candidateId)
  ]
);
const spendEvents = sqliteTable(
  "spend_events",
  {
    id: integer("id").primaryKey({ autoIncrement: true }),
    optimizationId: text("optimization_id").notNull().references(() => optimizations.id),
    agentId: text("agent_id").references(() => agents.id),
    category: text("category", { enum: SPEND_CATEGORIES }).notNull(),
    usd: real("usd").notNull(),
    estimated: integer("estimated", { mode: "boolean" }).notNull().default(true),
    source: text("source").notNull(),
    at: integer("at").notNull()
  },
  (t) => [
    index("spend_events_optimization_idx").on(t.optimizationId),
    check("spend_events_category_check", inList(t.category, SPEND_CATEGORIES))
  ]
);
const agentEvents = sqliteTable(
  "agent_events",
  {
    id: integer("id").primaryKey({ autoIncrement: true }),
    agentId: text("agent_id").notNull().references(() => agents.id),
    at: integer("at").notNull(),
    type: text("type").notNull(),
    payload: text("payload", { mode: "json" }).$type()
  },
  (t) => [index("agent_events_agent_idx").on(t.agentId, t.id)]
);
const schema = /* @__PURE__ */ Object.freeze(/* @__PURE__ */ Object.defineProperty({
  __proto__: null,
  AGENT_KINDS,
  AGENT_STATUSES,
  BENCH_PHASES,
  DIRECTIONS,
  OPTIMIZATION_STATUSES,
  SPEND_CATEGORIES,
  VERDICTS,
  agentEvents,
  agents,
  baselines,
  benchResults,
  candidates,
  optimizations,
  ratchetMoves,
  spendEvents
}, Symbol.toStringTag, { value: "Module" }));
class LedgerError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
    this.name = "LedgerError";
  }
  code;
}
const newId = (prefix) => `${prefix}_${crypto$1.randomUUID().replace(/-/g, "").slice(0, 16)}`;
function emptyCategories() {
  return Object.fromEntries(SPEND_CATEGORIES.map((c) => [c, 0]));
}
function assertFinite(name, v) {
  if (!Number.isFinite(v)) throw new LedgerError("invalid", `${name} must be a finite number`);
}
function isImprovement(direction, from, to) {
  return direction === "minimize" ? to < from : to > from;
}
function gainPercent(direction, from, to) {
  if (from === 0) return null;
  const delta = direction === "minimize" ? from - to : to - from;
  return delta / Math.abs(from) * 100;
}
class Ledger {
  constructor(db, sqlite, options = {}) {
    this.db = db;
    this.sqlite = sqlite;
    this.now = options.now ?? Date.now;
  }
  db;
  sqlite;
  now;
  close() {
    this.sqlite.close();
  }
  /** Run `fn` in a single SQLite transaction. */
  transaction(fn) {
    return this.sqlite.transaction(fn).immediate();
  }
  // ---- optimizations ------------------------------------------------------
  createOptimization(input) {
    assertFinite("minGain", input.minGain);
    assertFinite("budgetUsd", input.budgetUsd);
    if (input.minGain < 0) throw new LedgerError("invalid", "minGain must be >= 0");
    if (input.budgetUsd < 0) throw new LedgerError("invalid", "budgetUsd must be >= 0");
    const row = this.db.insert(optimizations).values({
      id: input.id ?? newId("opt"),
      name: input.name ?? null,
      repo: input.repo,
      baseRef: input.baseRef,
      metric: input.metric,
      direction: input.direction,
      minGain: input.minGain,
      budgetUsd: input.budgetUsd,
      timeLimitS: input.timeLimitS,
      status: input.status ?? "draft",
      createdAt: input.createdAt ?? this.now(),
      configYaml: input.configYaml
    }).returning().get();
    return row;
  }
  getOptimization(id2) {
    return this.db.select().from(optimizations).where(eq(optimizations.id, id2)).get();
  }
  /** Move an optimization through its lifecycle, stamping started/ended times. */
  setOptimizationStatus(id2, status, at = this.now()) {
    const cur = this.requireOptimization(id2);
    const patch = { status };
    if (status === "running" && cur.startedAt === null) patch.startedAt = at;
    if (status === "completed" || status === "stopped" || status === "failed") patch.endedAt = at;
    return this.db.update(optimizations).set(patch).where(eq(optimizations.id, id2)).returning().get();
  }
  /** Exactly what the Optimizations table shows, newest first. */
  listOptimizations(opts = {}) {
    const statuses = opts.status === void 0 ? void 0 : Array.isArray(opts.status) ? opts.status : [opts.status];
    const rows = this.db.select().from(optimizations).where(statuses ? inArray(optimizations.status, statuses) : void 0).orderBy(desc(optimizations.createdAt), desc(optimizations.id)).all();
    if (rows.length === 0) return [];
    const baselineByOpt = new Map(
      this.db.select({ id: baselines.optimizationId, value: baselines.value }).from(baselines).all().map((b) => [b.id, b.value])
    );
    const lastMoveByOpt = /* @__PURE__ */ new Map();
    const moveCountByOpt = /* @__PURE__ */ new Map();
    for (const m of this.db.select({ opt: ratchetMoves.optimizationId, to: ratchetMoves.toValue }).from(ratchetMoves).orderBy(asc(ratchetMoves.id)).all()) {
      lastMoveByOpt.set(m.opt, m.to);
      moveCountByOpt.set(m.opt, (moveCountByOpt.get(m.opt) ?? 0) + 1);
    }
    const spendByOpt = new Map(
      this.db.select({ opt: spendEvents.optimizationId, total: sql`coalesce(sum(${spendEvents.usd}), 0)` }).from(spendEvents).groupBy(spendEvents.optimizationId).all().map((s) => [s.opt, s.total])
    );
    const kindsByOpt = /* @__PURE__ */ new Map();
    const agentCountByOpt = /* @__PURE__ */ new Map();
    for (const a of this.db.select({ opt: agents.optimizationId, kind: agents.kind }).from(agents).all()) {
      if (!kindsByOpt.has(a.opt)) kindsByOpt.set(a.opt, /* @__PURE__ */ new Set());
      kindsByOpt.get(a.opt).add(a.kind);
      agentCountByOpt.set(a.opt, (agentCountByOpt.get(a.opt) ?? 0) + 1);
    }
    const candidateCountByOpt = new Map(
      this.db.select({ opt: candidates.optimizationId, n: sql`count(*)` }).from(candidates).groupBy(candidates.optimizationId).all().map((c) => [c.opt, c.n])
    );
    return rows.map((o) => {
      const baselineValue = baselineByOpt.get(o.id) ?? null;
      const bestValue = lastMoveByOpt.get(o.id) ?? baselineValue;
      const gainPct = baselineValue !== null && bestValue !== null ? gainPercent(o.direction, baselineValue, bestValue) : null;
      return {
        id: o.id,
        name: o.name,
        repo: o.repo,
        metric: o.metric,
        direction: o.direction,
        status: o.status,
        budgetUsd: o.budgetUsd,
        createdAt: o.createdAt,
        startedAt: o.startedAt,
        endedAt: o.endedAt,
        baselineValue,
        bestValue,
        gainPct,
        spendUsd: spendByOpt.get(o.id) ?? 0,
        agentKinds: [...kindsByOpt.get(o.id) ?? []].sort(),
        agentCount: agentCountByOpt.get(o.id) ?? 0,
        candidateCount: candidateCountByOpt.get(o.id) ?? 0,
        verifiedWins: moveCountByOpt.get(o.id) ?? 0
      };
    });
  }
  // ---- baseline -----------------------------------------------------------
  /** Record (or re-record, before any ratchet move) the baseline. */
  recordBaseline(input) {
    this.requireOptimization(input.optimizationId);
    assertFinite("noiseFloor", input.noiseFloor);
    const value = input.value ?? input.stats.value;
    assertFinite("baseline value", value);
    return this.transaction(() => {
      const moved = this.db.select({ id: ratchetMoves.id }).from(ratchetMoves).where(eq(ratchetMoves.optimizationId, input.optimizationId)).limit(1).get();
      if (moved) throw new LedgerError("invalid", "cannot change the baseline after the ratchet has moved");
      const values = {
        optimizationId: input.optimizationId,
        samples: input.samples,
        value,
        noiseFloor: input.noiseFloor,
        stats: input.stats,
        recordedAt: input.recordedAt ?? this.now()
      };
      return this.db.insert(baselines).values(values).onConflictDoUpdate({ target: baselines.optimizationId, set: values }).returning().get();
    });
  }
  getBaseline(optimizationId) {
    return this.db.select().from(baselines).where(eq(baselines.optimizationId, optimizationId)).get();
  }
  // ---- agents -------------------------------------------------------------
  addAgent(input) {
    this.requireOptimization(input.optimizationId);
    return this.db.insert(agents).values({
      id: input.id ?? newId("agt"),
      optimizationId: input.optimizationId,
      kind: input.kind,
      branch: input.branch,
      runnerInstanceId: input.runnerInstanceId ?? null,
      status: input.status ?? "pending",
      createdAt: input.createdAt ?? this.now()
    }).returning().get();
  }
  getAgent(id2) {
    return this.db.select().from(agents).where(eq(agents.id, id2)).get();
  }
  listAgents(optimizationId) {
    return this.db.select().from(agents).where(eq(agents.optimizationId, optimizationId)).orderBy(asc(agents.createdAt), asc(agents.id)).all();
  }
  setAgentStatus(id2, status, runnerInstanceId) {
    this.requireAgent(id2);
    const patch = { status };
    if (runnerInstanceId !== void 0) patch.runnerInstanceId = runnerInstanceId;
    return this.db.update(agents).set(patch).where(eq(agents.id, id2)).returning().get();
  }
  // ---- candidates ---------------------------------------------------------
  submitCandidate(input) {
    const agent = this.requireAgent(input.agentId);
    if (agent.optimizationId !== input.optimizationId) {
      throw new LedgerError("invalid", `agent ${input.agentId} does not belong to optimization ${input.optimizationId}`);
    }
    return this.db.insert(candidates).values({
      id: input.id ?? newId("cand"),
      optimizationId: input.optimizationId,
      agentId: input.agentId,
      sha: input.sha,
      message: input.message ?? "",
      submittedAt: input.submittedAt ?? this.now()
    }).returning().get();
  }
  getCandidate(id2) {
    return this.db.select().from(candidates).where(eq(candidates.id, id2)).get();
  }
  recordBenchResult(input) {
    this.requireCandidate(input.candidateId);
    assertFinite("stats.value", input.stats.value);
    return this.db.insert(benchResults).values({
      candidateId: input.candidateId,
      phase: input.phase,
      samples: input.samples,
      stats: input.stats,
      guardrails: input.guardrails ?? [],
      startedAt: input.startedAt,
      finishedAt: input.finishedAt
    }).returning().get();
  }
  setVerdict(candidateId, verdict, reasons = [], at = this.now()) {
    this.requireCandidate(candidateId);
    return this.db.update(candidates).set({ verdict, reasons, verdictAt: verdict === "pending" ? null : at }).where(eq(candidates.id, candidateId)).returning().get();
  }
  /** All candidates of an optimization (oldest first) with their bench/confirm results. */
  candidatesWithResults(optimizationId) {
    const rows = this.db.select({ candidate: candidates, agentKind: agents.kind, agentBranch: agents.branch }).from(candidates).innerJoin(agents, eq(agents.id, candidates.agentId)).where(eq(candidates.optimizationId, optimizationId)).orderBy(asc(candidates.submittedAt), asc(candidates.id)).all();
    if (rows.length === 0) return [];
    const ids = rows.map((r) => r.candidate.id);
    const resultsByCand = /* @__PURE__ */ new Map();
    for (const r of this.db.select().from(benchResults).where(inArray(benchResults.candidateId, ids)).orderBy(asc(benchResults.id)).all()) {
      const list = resultsByCand.get(r.candidateId) ?? [];
      list.push(r);
      resultsByCand.set(r.candidateId, list);
    }
    const moved = new Set(
      this.db.select({ id: ratchetMoves.candidateId }).from(ratchetMoves).where(eq(ratchetMoves.optimizationId, optimizationId)).all().map((m) => m.id)
    );
    return rows.map(({ candidate, agentKind: agentKind2, agentBranch }) => {
      const results = resultsByCand.get(candidate.id) ?? [];
      const latest = (phase) => {
        const r = results.filter((x) => x.phase === phase).at(-1);
        return r ? r.stats.value : null;
      };
      return {
        ...candidate,
        agentKind: agentKind2,
        agentBranch,
        results,
        benchValue: latest("bench"),
        confirmValue: latest("confirm"),
        movedRatchet: moved.has(candidate.id)
      };
    });
  }
  // ---- ratchet ------------------------------------------------------------
  /** Current ratchet value: last move's `to_value`, else the baseline, else null. */
  currentBest(optimizationId) {
    const last = this.db.select({ to: ratchetMoves.toValue }).from(ratchetMoves).where(eq(ratchetMoves.optimizationId, optimizationId)).orderBy(desc(ratchetMoves.id)).limit(1).get();
    if (last) return last.to;
    return this.getBaseline(optimizationId)?.value ?? null;
  }
  /**
   * Advance the ratchet to `toValue` (attributed to `candidateId`). The ratchet only
   * turns one way: rejects a value that is not strictly better than the current best
   * in the optimization's direction.
   */
  moveRatchet(optimizationId, candidateId, toValue, at = this.now()) {
    assertFinite("toValue", toValue);
    return this.transaction(() => {
      const opt = this.requireOptimization(optimizationId);
      const cand = this.requireCandidate(candidateId);
      if (cand.optimizationId !== optimizationId) {
        throw new LedgerError("invalid", `candidate ${candidateId} does not belong to optimization ${optimizationId}`);
      }
      const from = this.currentBest(optimizationId);
      if (from === null) throw new LedgerError("no_baseline", `optimization ${optimizationId} has no baseline yet`);
      if (!isImprovement(opt.direction, from, toValue)) {
        throw new LedgerError(
          "ratchet_not_improved",
          `${toValue} is not strictly better than current best ${from} (${opt.direction})`
        );
      }
      return this.db.insert(ratchetMoves).values({ optimizationId, candidateId, fromValue: from, toValue, at }).returning().get();
    });
  }
  ratchetHistory(optimizationId) {
    const opt = this.requireOptimization(optimizationId);
    return this.db.select({
      id: ratchetMoves.id,
      optimizationId: ratchetMoves.optimizationId,
      candidateId: ratchetMoves.candidateId,
      sha: candidates.sha,
      agentId: candidates.agentId,
      agentKind: agents.kind,
      fromValue: ratchetMoves.fromValue,
      toValue: ratchetMoves.toValue,
      at: ratchetMoves.at
    }).from(ratchetMoves).innerJoin(candidates, eq(candidates.id, ratchetMoves.candidateId)).innerJoin(agents, eq(agents.id, candidates.agentId)).where(eq(ratchetMoves.optimizationId, optimizationId)).orderBy(asc(ratchetMoves.id)).all().map((m) => ({ ...m, gainPct: gainPercent(opt.direction, m.fromValue, m.toValue) ?? 0 }));
  }
  // ---- spend --------------------------------------------------------------
  recordSpend(input) {
    assertFinite("usd", input.usd);
    this.requireOptimization(input.optimizationId);
    if (input.agentId) {
      const agent = this.requireAgent(input.agentId);
      if (agent.optimizationId !== input.optimizationId) {
        throw new LedgerError("invalid", `agent ${input.agentId} does not belong to optimization ${input.optimizationId}`);
      }
    }
    return this.db.insert(spendEvents).values({
      optimizationId: input.optimizationId,
      agentId: input.agentId ?? null,
      category: input.category,
      usd: input.usd,
      estimated: input.estimated ?? true,
      source: input.source,
      at: input.at ?? this.now()
    }).returning().get();
  }
  totalSpend(optimizationId) {
    const r = this.db.select({ total: sql`coalesce(sum(${spendEvents.usd}), 0)` }).from(spendEvents).where(eq(spendEvents.optimizationId, optimizationId)).get();
    return r?.total ?? 0;
  }
  spendBreakdown(optimizationId) {
    const rows = this.db.select({
      agentId: spendEvents.agentId,
      category: spendEvents.category,
      estimated: spendEvents.estimated,
      total: sql`sum(${spendEvents.usd})`
    }).from(spendEvents).where(eq(spendEvents.optimizationId, optimizationId)).groupBy(spendEvents.agentId, spendEvents.category, spendEvents.estimated).all();
    const out = { total: 0, estimated: 0, actual: 0, byCategory: emptyCategories(), byAgent: [] };
    const perAgent = /* @__PURE__ */ new Map();
    const agentInfo = new Map(this.listAgents(optimizationId).map((a) => [a.id, a]));
    for (const r of rows) {
      out.total += r.total;
      if (r.estimated) out.estimated += r.total;
      else out.actual += r.total;
      out.byCategory[r.category] += r.total;
      let entry = perAgent.get(r.agentId);
      if (!entry) {
        const info = r.agentId ? agentInfo.get(r.agentId) : void 0;
        entry = {
          agentId: r.agentId,
          kind: info?.kind ?? null,
          branch: info?.branch ?? null,
          total: 0,
          byCategory: emptyCategories()
        };
        perAgent.set(r.agentId, entry);
      }
      entry.total += r.total;
      entry.byCategory[r.category] += r.total;
    }
    for (const a of agentInfo.values()) {
      const e = perAgent.get(a.id);
      if (e) out.byAgent.push(e);
    }
    const shared = perAgent.get(null);
    if (shared) out.byAgent.push(shared);
    return out;
  }
  // ---- activity & agent events -------------------------------------------
  /** Merged, newest-first feed of verdicts and ratchet moves. */
  recentActivity(limit = 20, opts = {}) {
    if (limit <= 0) return [];
    const optId = opts.optimizationId;
    const verdicts = this.db.select({ c: candidates, agentKind: agents.kind }).from(candidates).innerJoin(agents, eq(agents.id, candidates.agentId)).where(and(ne(candidates.verdict, "pending"), optId ? eq(candidates.optimizationId, optId) : void 0)).orderBy(desc(candidates.verdictAt), desc(candidates.id)).limit(limit).all().map(
      ({ c, agentKind: agentKind2 }) => ({
        kind: "verdict",
        at: c.verdictAt ?? c.submittedAt,
        optimizationId: c.optimizationId,
        candidateId: c.id,
        agentId: c.agentId,
        agentKind: agentKind2,
        sha: c.sha,
        message: c.message,
        verdict: c.verdict,
        reasons: c.reasons
      })
    );
    const moves = this.db.select({ m: ratchetMoves, c: candidates, agentKind: agents.kind }).from(ratchetMoves).innerJoin(candidates, eq(candidates.id, ratchetMoves.candidateId)).innerJoin(agents, eq(agents.id, candidates.agentId)).where(optId ? eq(ratchetMoves.optimizationId, optId) : void 0).orderBy(desc(ratchetMoves.at), desc(ratchetMoves.id)).limit(limit).all().map(
      ({ m, c, agentKind: agentKind2 }) => ({
        kind: "ratchet_move",
        at: m.at,
        optimizationId: m.optimizationId,
        candidateId: c.id,
        agentId: c.agentId,
        agentKind: agentKind2,
        sha: c.sha,
        message: c.message,
        fromValue: m.fromValue,
        toValue: m.toValue
      })
    );
    return [...verdicts, ...moves].sort((a, b) => b.at - a.at || (a.kind === b.kind ? 0 : a.kind === "ratchet_move" ? -1 : 1)).slice(0, limit);
  }
  appendAgentEvent(input) {
    this.requireAgent(input.agentId);
    return this.db.insert(agentEvents).values({
      agentId: input.agentId,
      at: input.at ?? this.now(),
      type: input.type,
      payload: input.payload ?? null
    }).returning().get();
  }
  /** Events with id > `sinceId` (default: all), oldest first. Poll with the last seen id. */
  agentEvents(agentId, sinceId = 0, limit = 500) {
    return this.db.select().from(agentEvents).where(and(eq(agentEvents.agentId, agentId), gt(agentEvents.id, sinceId))).orderBy(asc(agentEvents.id)).limit(limit).all();
  }
  // ---- guards -------------------------------------------------------------
  requireOptimization(id2) {
    const row = this.getOptimization(id2);
    if (!row) throw new LedgerError("not_found", `optimization ${id2} not found`);
    return row;
  }
  requireAgent(id2) {
    const row = this.getAgent(id2);
    if (!row) throw new LedgerError("not_found", `agent ${id2} not found`);
    return row;
  }
  requireCandidate(id2) {
    const row = this.getCandidate(id2);
    if (!row) throw new LedgerError("not_found", `candidate ${id2} not found`);
    return row;
  }
}
class BetterSQLiteSession extends SQLiteSession {
  constructor(client, dialect, schema2, options = {}) {
    super(dialect);
    this.client = client;
    this.schema = schema2;
    this.logger = options.logger ?? new NoopLogger();
    this.cache = options.cache ?? new NoopCache();
  }
  static [entityKind] = "BetterSQLiteSession";
  logger;
  cache;
  prepareQuery(query, fields, executeMethod, isResponseInArrayMode, customResultMapper, queryMetadata, cacheConfig) {
    const stmt = this.client.prepare(query.sql);
    return new PreparedQuery(
      stmt,
      query,
      this.logger,
      this.cache,
      queryMetadata,
      cacheConfig,
      fields,
      executeMethod,
      isResponseInArrayMode,
      customResultMapper
    );
  }
  transaction(transaction, config = {}) {
    const tx = new BetterSQLiteTransaction("sync", this.dialect, this, this.schema);
    const nativeTx = this.client.transaction(transaction);
    return nativeTx[config.behavior ?? "deferred"](tx);
  }
}
class BetterSQLiteTransaction extends SQLiteTransaction {
  static [entityKind] = "BetterSQLiteTransaction";
  transaction(transaction) {
    const savepointName = `sp${this.nestedIndex}`;
    const tx = new BetterSQLiteTransaction("sync", this.dialect, this.session, this.schema, this.nestedIndex + 1);
    this.session.run(sql.raw(`savepoint ${savepointName}`));
    try {
      const result = transaction(tx);
      this.session.run(sql.raw(`release savepoint ${savepointName}`));
      return result;
    } catch (err) {
      this.session.run(sql.raw(`rollback to savepoint ${savepointName}`));
      throw err;
    }
  }
}
class PreparedQuery extends SQLitePreparedQuery {
  constructor(stmt, query, logger, cache, queryMetadata, cacheConfig, fields, executeMethod, _isResponseInArrayMode, customResultMapper) {
    super("sync", executeMethod, query, cache, queryMetadata, cacheConfig);
    this.stmt = stmt;
    this.logger = logger;
    this.fields = fields;
    this._isResponseInArrayMode = _isResponseInArrayMode;
    this.customResultMapper = customResultMapper;
  }
  static [entityKind] = "BetterSQLitePreparedQuery";
  run(placeholderValues) {
    const params = fillPlaceholders(this.query.params, placeholderValues ?? {});
    this.logger.logQuery(this.query.sql, params);
    return this.stmt.run(...params);
  }
  all(placeholderValues) {
    const { fields, joinsNotNullableMap, query, logger, stmt, customResultMapper } = this;
    if (!fields && !customResultMapper) {
      const params = fillPlaceholders(query.params, placeholderValues ?? {});
      logger.logQuery(query.sql, params);
      return stmt.all(...params);
    }
    const rows = this.values(placeholderValues);
    if (customResultMapper) {
      return customResultMapper(rows);
    }
    return rows.map((row) => mapResultRow(fields, row, joinsNotNullableMap));
  }
  get(placeholderValues) {
    const params = fillPlaceholders(this.query.params, placeholderValues ?? {});
    this.logger.logQuery(this.query.sql, params);
    const { fields, stmt, joinsNotNullableMap, customResultMapper } = this;
    if (!fields && !customResultMapper) {
      return stmt.get(...params);
    }
    const row = stmt.raw().get(...params);
    if (!row) {
      return void 0;
    }
    if (customResultMapper) {
      return customResultMapper([row]);
    }
    return mapResultRow(fields, row, joinsNotNullableMap);
  }
  values(placeholderValues) {
    const params = fillPlaceholders(this.query.params, placeholderValues ?? {});
    this.logger.logQuery(this.query.sql, params);
    return this.stmt.raw().all(...params);
  }
  /** @internal */
  isResponseInArrayMode() {
    return this._isResponseInArrayMode;
  }
}
class BetterSQLite3Database extends BaseSQLiteDatabase {
  static [entityKind] = "BetterSQLite3Database";
}
function construct(client, config = {}) {
  const dialect = new SQLiteSyncDialect({ casing: config.casing });
  let logger;
  if (config.logger === true) {
    logger = new DefaultLogger();
  } else if (config.logger !== false) {
    logger = config.logger;
  }
  let schema2;
  if (config.schema) {
    const tablesConfig = extractTablesRelationalConfig(
      config.schema,
      createTableRelationsHelpers
    );
    schema2 = {
      fullSchema: config.schema,
      schema: tablesConfig.tables,
      tableNamesMap: tablesConfig.tableNamesMap
    };
  }
  const session = new BetterSQLiteSession(client, dialect, schema2, { logger });
  const db = new BetterSQLite3Database("sync", dialect, session, schema2);
  db.$client = client;
  return db;
}
function drizzle(...params) {
  if (params[0] === void 0 || typeof params[0] === "string") {
    const instance = params[0] === void 0 ? new Client() : new Client(params[0]);
    return construct(instance, params[1]);
  }
  if (isConfig(params[0])) {
    const { connection, client, ...drizzleConfig } = params[0];
    if (client) return construct(client, drizzleConfig);
    if (typeof connection === "object") {
      const { source, ...options } = connection;
      const instance2 = new Client(source, options);
      return construct(instance2, drizzleConfig);
    }
    const instance = new Client(connection);
    return construct(instance, drizzleConfig);
  }
  return construct(params[0], params[1]);
}
((drizzle2) => {
  function mock(config) {
    return construct({}, config);
  }
  drizzle2.mock = mock;
})(drizzle || (drizzle = {}));
function readMigrationFiles(config) {
  const migrationFolderTo = config.migrationsFolder;
  const migrationQueries = [];
  const journalPath = `${migrationFolderTo}/meta/_journal.json`;
  if (!fs.existsSync(journalPath)) {
    throw new Error(`Can't find meta/_journal.json file`);
  }
  const journalAsString = fs.readFileSync(`${migrationFolderTo}/meta/_journal.json`).toString();
  const journal = JSON.parse(journalAsString);
  for (const journalEntry of journal.entries) {
    const migrationPath = `${migrationFolderTo}/${journalEntry.tag}.sql`;
    try {
      const query = fs.readFileSync(`${migrationFolderTo}/${journalEntry.tag}.sql`).toString();
      const result = query.split("--> statement-breakpoint").map((it) => {
        return it;
      });
      migrationQueries.push({
        sql: result,
        bps: journalEntry.breakpoints,
        folderMillis: journalEntry.when,
        hash: crypto$1.createHash("sha256").update(query).digest("hex")
      });
    } catch {
      throw new Error(`No file ${migrationPath} found in ${migrationFolderTo} folder`);
    }
  }
  return migrationQueries;
}
function migrate(db, config) {
  const migrations = readMigrationFiles(config);
  db.dialect.migrate(migrations, db.session, config);
}
function resolveMigrationsFolder() {
  const candidates2 = [
    new URL("./migrations", require("url").pathToFileURL(__filename).href),
    new URL("../../src/ledger/migrations", require("url").pathToFileURL(__filename).href)
  ];
  for (const url of candidates2) {
    const p = node_url.fileURLToPath(url);
    if (fs.existsSync(p)) return p;
  }
  throw new Error("ratchet ledger: migrations folder not found");
}
function openLedger(path = ":memory:", options = {}) {
  const sqlite = new Client(path);
  try {
    sqlite.pragma("foreign_keys = ON");
    if (path !== ":memory:") sqlite.pragma("journal_mode = WAL");
    sqlite.pragma("busy_timeout = 5000");
    const db = drizzle(sqlite, { schema });
    migrate(db, { migrationsFolder: options.migrationsFolder ?? resolveMigrationsFolder() });
    return new Ledger(db, sqlite, options);
  } catch (err) {
    sqlite.close();
    throw err;
  }
}
const IPC = {
  listOptimizations: "ratchet:listOptimizations",
  getOptimization: "ratchet:getOptimization",
  ratchetHistory: "ratchet:ratchetHistory",
  candidatesWithResults: "ratchet:candidatesWithResults",
  spendBreakdown: "ratchet:spendBreakdown",
  recentActivity: "ratchet:recentActivity",
  agentEvents: "ratchet:agentEvents",
  validateConfig: "ratchet:validateConfig",
  preflight: "ratchet:preflight",
  sandboxInfo: "ratchet:sandboxInfo",
  getSettings: "ratchet:getSettings",
  createOptimization: "ratchet:createOptimization",
  startOptimization: "ratchet:startOptimization",
  stopOptimization: "ratchet:stopOptimization",
  pauseAgent: "ratchet:pauseAgent",
  resumeAgent: "ratchet:resumeAgent",
  messageAgent: "ratchet:messageAgent",
  openPullRequest: "ratchet:openPullRequest",
  teardown: "ratchet:teardown",
  exportReport: "ratchet:exportReport",
  setMode: "ratchet:setMode",
  setAwsProfile: "ratchet:setAwsProfile",
  setSecret: "ratchet:setSecret",
  clearSecret: "ratchet:clearSecret",
  event: "ratchet:event"
};
const DEFAULT_CONFIG_YAML = `version: 0
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
const HYPOTHESES = [
  "Memoise the tokenizer so repeated queries skip re-parsing",
  "Batch index reads to cut round trips on the hot path",
  "Replace the JSON response encoder with a pre-sized buffer writer",
  "Cache compiled query plans keyed by normalised query",
  "Avoid re-sorting results that the index already returns ordered",
  "Move the facet counting off the request path into a lazy iterator",
  "Switch the scoring loop to typed arrays to reduce GC pressure",
  "Short-circuit empty queries before they reach the index"
];
const PROTECTED_TOUCHES = ["bench/k6/search.js", "tests/contract/search.test.ts"];
function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = a + 1831565813 >>> 0;
    let t = a;
    t = Math.imul(t ^ t >>> 15, t | 1);
    t ^= t + Math.imul(t ^ t >>> 7, t | 61);
    return ((t ^ t >>> 14) >>> 0) / 4294967296;
  };
}
class DevEngine {
  constructor(ledger, opts) {
    this.ledger = ledger;
    this.modeValue = opts.mode;
    this.rand = mulberry32(opts.seed ?? Date.now());
    this.now = opts.now ?? Date.now;
    this.tickMs = opts.tickMs ?? 2500;
    this.baselineMs = opts.baselineMs ?? 1200;
    this.autoRun = opts.autoRun ?? true;
  }
  ledger;
  listeners = /* @__PURE__ */ new Set();
  runs = /* @__PURE__ */ new Map();
  paused = /* @__PURE__ */ new Set();
  inbox = /* @__PURE__ */ new Map();
  rand;
  now;
  tickMs;
  baselineMs;
  autoRun;
  modeValue;
  pending = [];
  mode() {
    return this.modeValue;
  }
  subscribe(listener) {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  /** Stop all timers (app quit / mode switch). Ledger state is untouched. */
  dispose() {
    for (const r of this.runs.values()) if (r.timer) clearInterval(r.timer);
    for (const t of this.pending) clearTimeout(t);
    this.runs.clear();
    this.pending = [];
    this.listeners.clear();
  }
  // ------------------------------------------------------------ EngineApi
  async createOptimization(input) {
    const parsed = parseConfig(input.configYaml ?? DEFAULT_CONFIG_YAML);
    if (!parsed.ok) throw new Error(`invalid ratchet.yaml: ${parsed.errors.map((e) => e.message).join("; ")}`);
    const cfg = parsed.config;
    const repo = repoSlug(input.repoUrl);
    const opt = this.ledger.createOptimization({
      name: input.name ?? `${repo.split("/").pop()} ${cfg.objective.metric}`,
      repo,
      baseRef: input.baseRef,
      metric: cfg.objective.metric,
      direction: cfg.objective.direction,
      minGain: cfg.objective.min_gain,
      budgetUsd: input.budgetUsd,
      timeLimitS: input.timeLimitSeconds,
      configYaml: input.configYaml ?? DEFAULT_CONFIG_YAML,
      status: "draft"
    });
    input.agents.forEach((kind, i) => {
      this.ledger.addAgent({ optimizationId: opt.id, kind, branch: `ratchet/${opt.id.slice(4, 12)}/${kind}-${i + 1}` });
    });
    this.plateauByOpt.set(opt.id, input.plateauEvals);
    this.emit({ type: "optimization", optimizationId: opt.id, status: "draft" });
    return opt.id;
  }
  async start(id2) {
    const opt = this.ledger.getOptimization(id2);
    if (!opt) throw new Error(`optimization ${id2} not found`);
    if (this.runs.has(id2)) return;
    const parsed = parseConfig(opt.configYaml);
    if (!parsed.ok) throw new Error("stored ratchet.yaml is invalid");
    const run = { timer: null, config: parsed.config, noEval: 0, turn: 0 };
    this.runs.set(id2, run);
    if (this.ledger.getBaseline(id2)) {
      this.beginRunning(id2, run);
      return;
    }
    this.ledger.setOptimizationStatus(id2, "baselining");
    for (const a of this.ledger.listAgents(id2)) this.ledger.setAgentStatus(a.id, "launching", `i-sim${a.id.slice(-8)}`);
    this.emit({ type: "optimization", optimizationId: id2, status: "baselining" });
    const finish = () => {
      this.recordBaseline(id2, run);
      this.beginRunning(id2, run);
    };
    if (this.autoRun) this.pending.push(setTimeout(finish, this.baselineMs));
    else finish();
  }
  async stop(id2, _reason) {
    const opt = this.ledger.getOptimization(id2);
    if (!opt) throw new Error(`optimization ${id2} not found`);
    this.halt(id2);
    this.ledger.setOptimizationStatus(id2, "stopping");
    for (const a of this.ledger.listAgents(id2)) this.ledger.setAgentStatus(a.id, "stopped");
    this.ledger.setOptimizationStatus(id2, "stopped");
    this.emit({ type: "optimization", optimizationId: id2, status: "stopped" });
  }
  async pauseAgent(agentId) {
    this.requireAgent(agentId);
    this.paused.add(agentId);
    this.ledger.setAgentStatus(agentId, "paused");
    this.logEvent(agentId, { type: "status", text: "Paused by user" });
  }
  async resumeAgent(agentId) {
    this.requireAgent(agentId);
    this.paused.delete(agentId);
    this.ledger.setAgentStatus(agentId, "running");
    this.logEvent(agentId, { type: "status", text: "Resumed" });
  }
  async messageAgent(agentId, text2) {
    this.requireAgent(agentId);
    const q = this.inbox.get(agentId) ?? [];
    q.push(text2);
    this.inbox.set(agentId, q);
    this.logEvent(agentId, { type: "message", text: `You: ${text2}`, data: { from: "user" } });
  }
  async openPullRequest(id2) {
    const opt = this.ledger.getOptimization(id2);
    if (!opt) throw new Error(`optimization ${id2} not found`);
    if (this.ledger.ratchetHistory(id2).length === 0) throw new Error("no verified wins to open a pull request for");
    return { url: `https://github.com/${opt.repo}/pull/${100 + id2.charCodeAt(id2.length - 1) % 800}` };
  }
  async teardown(id2) {
    if (!this.ledger.getOptimization(id2)) throw new Error(`optimization ${id2} not found`);
    this.halt(id2);
  }
  // ------------------------------------------------------------ simulation
  /** Run one simulated agent turn for every running agent of `id` (called by the timer; public for tests). */
  tick(id2) {
    const run = this.runs.get(id2);
    const opt = this.ledger.getOptimization(id2);
    if (!run || !opt || opt.status !== "running") return;
    run.turn += 1;
    const elapsed = this.now() - (opt.startedAt ?? opt.createdAt);
    const spent = this.ledger.totalSpend(id2);
    if (spent >= opt.budgetUsd) return this.finish(id2, "budget exhausted");
    if (elapsed >= opt.timeLimitS * 1e3) return this.finish(id2, "time limit reached");
    if (run.noEval >= this.plateau(id2)) return this.finish(id2, "plateau: no verified win in the last evaluations");
    for (const agent of this.ledger.listAgents(id2)) {
      if (agent.status !== "running") continue;
      this.agentTurn(id2, run, agent.id, agent.kind);
      if (this.ledger.getOptimization(id2)?.status !== "running") return;
    }
  }
  /** Plateau stop-rule (consecutive non-improving evaluations), kept in memory: the ledger has no column for it. */
  plateauByOpt = /* @__PURE__ */ new Map();
  plateau(id2) {
    return this.plateauByOpt.get(id2) ?? 25;
  }
  recordBaseline(id2, run) {
    const max = run.config.objective.direction === "maximize";
    const center = max ? 40 + this.rand() * 10 : 150 + this.rand() * 60;
    const samples = Array.from({ length: Math.max(5, run.config.bench.repeats) }, () => round(center * (1 + (this.rand() - 0.5) * 0.04)));
    const s = summarize(samples);
    const t = this.now();
    this.ledger.recordBaseline({
      optimizationId: id2,
      samples,
      noiseFloor: noiseFloor(samples),
      stats: { value: s.median, p50: s.median, p95: round(s.median * 1.18), p99: round(s.median * 1.42), mean: s.mean },
      recordedAt: t
    });
    this.ledger.recordSpend({ optimizationId: id2, category: "bench_runner", usd: 0.62, source: "dev-engine:baseline", at: t });
  }
  beginRunning(id2, run) {
    this.ledger.setOptimizationStatus(id2, "running");
    for (const a of this.ledger.listAgents(id2)) {
      if (a.status !== "running" && a.status !== "paused") this.ledger.setAgentStatus(a.id, "running", a.runnerInstanceId ?? `i-sim${a.id.slice(-8)}`);
      this.logEvent(a.id, { type: "status", text: "Agent runner online; reading RATCHET.md" });
    }
    this.emit({ type: "optimization", optimizationId: id2, status: "running" });
    if (this.autoRun) run.timer = setInterval(() => this.tick(id2), this.tickMs);
  }
  agentTurn(id2, run, agentId, kind) {
    const opt = this.ledger.getOptimization(id2);
    const cfg = run.config;
    const baseline = this.ledger.getBaseline(id2);
    const best = this.ledger.currentBest(id2) ?? baseline.value;
    const dir = cfg.objective.direction;
    const sign = dir === "minimize" ? -1 : 1;
    const queued = this.inbox.get(agentId)?.shift();
    const hypothesis = queued ? `Following your note: ${queued}` : HYPOTHESES[Math.floor(this.rand() * HYPOTHESES.length)];
    this.logEvent(agentId, { type: "message", text: hypothesis });
    this.logEvent(agentId, { type: "tool", text: "edit src/search/index.ts" });
    const sha = Array.from({ length: 40 }, () => Math.floor(this.rand() * 16).toString(16)).join("");
    this.logEvent(agentId, { type: "commit", text: `Ratchet-Candidate: ${sha.slice(0, 7)}`, data: { sha } });
    const t0 = this.now();
    const cand = this.ledger.submitCandidate({ optimizationId: id2, agentId, sha, message: hypothesis, submittedAt: t0 });
    const r = this.rand();
    const effect = r < 0.22 ? 0.04 + this.rand() * 0.05 : r < 0.75 ? (this.rand() - 0.4) * 0.03 : -0.02 - this.rand() * 0.03;
    const centre = best * (1 + sign * effect);
    const sample = (c) => round(c * (1 + (this.rand() - 0.5) * 0.03));
    const samples = Array.from({ length: cfg.bench.repeats }, () => sample(centre));
    const guardrails = cfg.guardrails.map((g) => {
      const name = g.kind === "command" ? g.run : `${g.metric} ${g.raw}`;
      return this.rand() < 0.04 ? { name, passed: false, detail: "regressed" } : { name, passed: true };
    });
    const touched = this.rand() < 0.04 ? [PROTECTED_TOUCHES[Math.floor(this.rand() * PROTECTED_TOUCHES.length)]] : [];
    const protectedViolations = touched.map((path) => ({ path, pattern: path.startsWith("bench") ? "bench/**" : "tests/contract/**" }));
    const stats = (xs) => {
      const s = summarize(xs);
      return { value: s.median, p50: s.median, p95: round(s.median * 1.18), p99: round(s.median * 1.42), mean: s.mean };
    };
    const input = { direction: dir, minGain: opt.minGain, noiseFloor: baseline.noiseFloor, currentBest: best, candidate: samples, guardrails, protectedViolations };
    const bench0 = this.now();
    let outcome = evaluateRatchet(input);
    this.ledger.recordBenchResult({ candidateId: cand.id, phase: "bench", samples, stats: stats(samples), guardrails, startedAt: bench0, finishedAt: this.now() });
    this.spend(id2, agentId, "bench_runner", 0.18 + this.rand() * 0.12, "dev-engine:bench");
    this.spend(id2, agentId, "tokens", 0.25 + this.rand() * 0.55, `dev-engine:${kind}`);
    this.spend(id2, agentId, "agent_runner", 0.06 + this.rand() * 0.04, "dev-engine:runner");
    this.spend(id2, agentId, "experiment", this.rand() * 0.1, "dev-engine:experiment");
    if (this.rand() < 0.2) this.spend(id2, agentId, "storage_egress", 0.01, "dev-engine:s3");
    if (outcome.verdict === "pending_confirm") {
      const holds = this.rand() > 0.15;
      const confirm = Array.from({ length: cfg.bench.repeats }, () => sample(holds ? centre : best * (1 + -sign * 0.01)));
      const c0 = this.now();
      outcome = evaluateRatchet({ ...input, confirm });
      this.ledger.recordBenchResult({ candidateId: cand.id, phase: "confirm", samples: confirm, stats: stats(confirm), guardrails: [], startedAt: c0, finishedAt: this.now() });
      this.spend(id2, agentId, "bench_runner", 0.2, "dev-engine:confirm");
    }
    const verdict = outcome.verdict === "pending_confirm" ? "rejected" : outcome.verdict;
    this.ledger.setVerdict(cand.id, verdict, outcome.reasons);
    this.emit({ type: "candidate", optimizationId: id2, candidateId: cand.id, verdict });
    if (verdict === "verified") {
      this.ledger.moveRatchet(id2, cand.id, outcome.candidateValue);
      this.emit({ type: "ratchet", optimizationId: id2, from: best, to: outcome.candidateValue });
      run.noEval = 0;
    } else {
      run.noEval += 1;
    }
    this.logEvent(agentId, {
      type: "verdict",
      text: `${verdict}: ${outcome.reasons[0] ?? ""}`,
      data: { sha, verdict, value: outcome.candidateValue }
    });
    this.emit({ type: "spend", optimizationId: id2, totalUsd: this.ledger.totalSpend(id2) });
  }
  finish(id2, why) {
    this.halt(id2);
    for (const a of this.ledger.listAgents(id2)) {
      this.ledger.setAgentStatus(a.id, "done");
      this.logEvent(a.id, { type: "status", text: `Stopped: ${why}` });
    }
    this.ledger.setOptimizationStatus(id2, "completed");
    this.emit({ type: "optimization", optimizationId: id2, status: "completed" });
  }
  halt(id2) {
    const run = this.runs.get(id2);
    if (run?.timer) clearInterval(run.timer);
    this.runs.delete(id2);
  }
  spend(id2, agentId, category, usd, source) {
    this.ledger.recordSpend({ optimizationId: id2, agentId, category, usd: round(usd, 4), source, at: this.now() });
  }
  logEvent(agentId, e) {
    const at = this.now();
    const optimizationId = this.ledger.getAgent(agentId).optimizationId;
    this.ledger.appendAgentEvent({ agentId, type: e.type, payload: { text: e.text, data: e.data }, at });
    this.emit({ type: "agent-event", optimizationId, agentId, event: { at, ...e } });
  }
  requireAgent(agentId) {
    if (!this.ledger.getAgent(agentId)) throw new Error(`agent ${agentId} not found`);
  }
  emit(e) {
    for (const l of this.listeners) {
      try {
        l(e);
      } catch {
      }
    }
  }
}
const round = (n, dp = 2) => Number(n.toFixed(dp));
function repoSlug(repoUrl) {
  const m = /([\w.-]+\/[\w.-]+?)(?:\.git)?\/?$/.exec(repoUrl.trim());
  return m?.[1] ?? repoUrl.trim();
}
class DevSandboxProvider {
  applied = /* @__PURE__ */ new Map();
  async plan(input) {
    const exists2 = this.applied.has(input.sandboxId);
    const act = (a) => exists2 ? "no-op" : a;
    const changes = [
      { action: act("create"), address: "aws_s3_bucket.artifacts", type: "aws_s3_bucket" },
      { action: act("create"), address: "aws_s3_bucket.sealed", type: "aws_s3_bucket" },
      { action: act("create"), address: "aws_s3_bucket_public_access_block.sealed", type: "aws_s3_bucket_public_access_block" },
      { action: act("create"), address: "aws_iam_policy.agent_boundary", type: "aws_iam_policy" },
      { action: act("create"), address: "aws_iam_role.eval_identity", type: "aws_iam_role" },
      { action: act("create"), address: "aws_vpc.sandbox", type: "aws_vpc" },
      { action: act("create"), address: "aws_subnet.runners", type: "aws_subnet" },
      { action: act("create"), address: "aws_security_group.agent_runner", type: "aws_security_group" },
      { action: act("create"), address: "aws_security_group.bench", type: "aws_security_group" },
      { action: act("update"), address: "aws_budgets_budget.sandbox", type: "aws_budgets_budget" }
    ];
    const n = (a) => changes.filter((c) => c.action === a).length;
    return { changes, summary: `Plan: ${n("create")} to add, ${n("update")} to change, ${n("delete")} to destroy.` };
  }
  async apply(input) {
    const out = this.outputsFor(input.sandboxId);
    this.applied.set(input.sandboxId, out);
    return out;
  }
  async destroy(input) {
    this.applied.delete(input.sandboxId);
  }
  async outputs(sandboxId) {
    return this.applied.get(sandboxId) ?? this.outputsFor(sandboxId);
  }
  outputsFor(id2) {
    return {
      artifactsBucket: `ratchet-${id2}-artifacts`,
      sealedBucket: `ratchet-${id2}-sealed`,
      agentBoundaryRef: `arn:aws:iam::000000000000:policy/ratchet-${id2}-agent-boundary`,
      evalIdentityRef: `arn:aws:iam::000000000000:role/ratchet-${id2}-eval`,
      agentRolePath: `/ratchet/${id2}/agents/`,
      network: { subnetId: "subnet-0a1b2c3d4e5f60718", agentRunnerSecurityGroupId: "sg-0123456789abcdef0", benchSecurityGroupId: "sg-0fedcba9876543210" }
    };
  }
}
const MIN$1 = 6e4;
const around = (v) => [0, 4e-3, -3e-3, 2e-3, -5e-3].map((d) => Number((v * (1 + d)).toFixed(3)));
const SPECS = [
  {
    id: "opt_demo_search",
    name: "search-api p95 latency",
    repo: "acme/search-api",
    metric: "http_req_duration.p95",
    direction: "minimize",
    minGain: 0.03,
    baseline: 182,
    noise: 3.1,
    agents: ["claude-code", "codex"],
    status: "running",
    startedAgo: 224,
    spendUsd: 42.6,
    budgetUsd: 120,
    candidates: [
      { t: 14, agent: 1, value: 179.5, verdict: "rejected", message: "Inline hot-path helper in query parser", reason: "improvement 2.5 does not exceed required gain 5.46 (max of noise floor 3.1 and min_gain 5.46)" },
      { t: 31, agent: 0, value: 168, verdict: "verified", confirm: 169.4, message: "Cache compiled query plans keyed by normalised query", reason: "bench improvement 14 exceeds required gain 5.46" },
      { t: 55, agent: 1, value: 171, verdict: "rejected", message: "Pool JSON encoder buffers", reason: "candidate 171 is not better than current best 168 (minimize)" },
      { t: 78, agent: 1, value: 156, verdict: "verified", confirm: 158.2, message: "Batch index reads to cut round trips", reason: "bench improvement 12 exceeds required gain 5.04" },
      { t: 96, agent: 0, value: 148, verdict: "failed_confirm", confirm: 160.1, message: "Skip scoring for zero-weight terms", reason: "confirm run 160.1 does not beat current best 156 on the hidden trace (minimize)" },
      { t: 112, agent: 0, value: 149, verdict: "verified", confirm: 150.7, message: "Memoise tokenizer output per request", reason: "bench improvement 7 exceeds required gain 4.68" },
      { t: 125, agent: 1, value: 142, verdict: "blocked", message: "Tighten bench warm-up window", reason: "protected path changed: bench/k6/search.js (matches bench/**)" },
      { t: 140, agent: 0, value: 152, verdict: "rejected", message: "Pre-size result arrays", reason: "candidate 152 is not better than current best 149 (minimize)" },
      { t: 171, agent: 1, value: 138, verdict: "verified", confirm: 140.3, message: "Stream facet counts lazily", reason: "bench improvement 11 exceeds required gain 4.47" },
      { t: 190, agent: 0, value: 140, verdict: "rejected", message: "Typed arrays in the scoring loop", reason: "candidate 140 is not better than current best 138 (minimize)" },
      { t: 214, agent: 0, value: 131, verdict: "verified", confirm: 133, message: "Short-circuit empty queries before the index", reason: "bench improvement 7 exceeds required gain 4.14" }
    ]
  },
  {
    id: "opt_demo_ingest",
    name: "Ingest throughput",
    repo: "acme/ingest",
    metric: "throughput.events_per_s",
    direction: "maximize",
    minGain: 0.03,
    baseline: 41.2,
    noise: 0.9,
    agents: ["codex"],
    status: "completed",
    startedAgo: 24 * 60 + 90,
    endedAgo: 120,
    spendUsd: 61.2,
    budgetUsd: 100,
    candidates: [
      { t: 90, agent: 0, value: 49.8, verdict: "verified", confirm: 49.1, message: "Batch writes into 4 KiB segments", reason: "bench improvement 8.6 exceeds required gain 1.24" },
      { t: 170, agent: 0, value: 50.4, verdict: "rejected", message: "Larger socket buffers", reason: "improvement 0.6 does not exceed required gain 1.49" },
      { t: 260, agent: 0, value: 58.9, verdict: "verified", confirm: 57.6, message: "Vectorise the parser inner loop", reason: "bench improvement 9.1 exceeds required gain 1.49" }
    ]
  },
  {
    id: "opt_demo_edge",
    name: "Edge worker cold start",
    repo: "acme/edge-worker",
    metric: "cold_start.s",
    direction: "minimize",
    minGain: 0.03,
    baseline: 2.41,
    noise: 0.05,
    agents: ["claude-code", "codex"],
    status: "completed",
    startedAgo: 48 * 60,
    endedAgo: 26 * 60,
    spendUsd: 38.9,
    budgetUsd: 60,
    candidates: [{ t: 120, agent: 1, value: 1.62, verdict: "verified", confirm: 1.66, message: "Lazy-load the template engine", reason: "bench improvement 0.79 exceeds required gain 0.072" }]
  },
  {
    id: "opt_demo_web",
    name: "Web render p99",
    repo: "acme/web",
    metric: "render.p99",
    direction: "minimize",
    minGain: 0.03,
    baseline: 0,
    noise: 0,
    agents: [],
    status: "draft",
    startedAgo: 72 * 60,
    spendUsd: 0,
    budgetUsd: 50,
    candidates: []
  }
];
function isLedgerEmpty(ledger) {
  return ledger.listOptimizations().length === 0;
}
function seedDemo(ledger, now = Date.now()) {
  for (const spec of SPECS) seedOne(ledger, spec, now);
}
function seedOne(ledger, spec, now) {
  const started = now - spec.startedAgo * MIN$1;
  const yaml2 = DEFAULT_CONFIG_YAML.replace("http_req_duration.p95", spec.metric).replace(
    "direction: minimize",
    `direction: ${spec.direction}`
  );
  ledger.createOptimization({
    id: spec.id,
    name: spec.name,
    repo: spec.repo,
    baseRef: "main",
    metric: spec.metric,
    direction: spec.direction,
    minGain: spec.minGain,
    budgetUsd: spec.budgetUsd,
    timeLimitS: 8 * 3600,
    configYaml: yaml2,
    status: "draft",
    createdAt: started - 5 * MIN$1
  });
  if (spec.status === "draft") return;
  ledger.setOptimizationStatus(spec.id, "baselining", started);
  const samples = around(spec.baseline);
  ledger.recordBaseline({
    optimizationId: spec.id,
    samples,
    noiseFloor: spec.noise,
    stats: { value: spec.baseline, p50: spec.baseline, p95: Number((spec.baseline * 1.18).toFixed(2)), p99: Number((spec.baseline * 1.42).toFixed(2)) },
    recordedAt: started + 2 * MIN$1
  });
  ledger.setOptimizationStatus(spec.id, "running", started + 3 * MIN$1);
  const agentIds = spec.agents.map(
    (kind, i) => ledger.addAgent({
      id: `${spec.id}_agt_${i}`,
      optimizationId: spec.id,
      kind,
      branch: `ratchet/${spec.id.slice(9)}/${kind}-${i + 1}`,
      status: "running",
      runnerInstanceId: `i-0${spec.id.length}a${i}c9e1b7d`,
      createdAt: started
    }).id
  );
  for (const [i, id2] of agentIds.entries()) {
    ledger.appendAgentEvent({ agentId: id2, type: "status", payload: { text: "Agent runner online; reading RATCHET.md" }, at: started + (4 + i) * MIN$1 });
  }
  const parts = [
    { cat: "tokens", w: 0.55 },
    { cat: "bench_runner", w: 0.3 },
    { cat: "agent_runner", w: 0.09 },
    { cat: "experiment", w: 0.05 },
    { cat: "storage_egress", w: 0.01 }
  ];
  const wSum = parts.reduce((a, p) => a + p.w, 0) * Math.max(1, spec.candidates.length);
  const scale = spec.spendUsd / wSum;
  let best = spec.baseline;
  for (const [n, c] of spec.candidates.entries()) {
    const agentId = agentIds[c.agent];
    const at = started + c.t * MIN$1;
    const sha = (Math.abs(hash(`${spec.id}${n}`)).toString(16) + "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678").slice(0, 40);
    ledger.appendAgentEvent({ agentId, type: "message", payload: { text: c.message }, at: at - 4 * MIN$1 });
    ledger.appendAgentEvent({ agentId, type: "tool", payload: { text: "edit src/search/index.ts" }, at: at - 3 * MIN$1 });
    ledger.appendAgentEvent({ agentId, type: "commit", payload: { text: `Ratchet-Candidate: ${sha.slice(0, 7)}`, data: { sha } }, at: at - 1 * MIN$1 });
    const cand = ledger.submitCandidate({ optimizationId: spec.id, agentId, sha, message: c.message, submittedAt: at });
    const stat = (v) => ({ value: v, p50: v, p95: Number((v * 1.18).toFixed(2)), p99: Number((v * 1.42).toFixed(2)) });
    const blocked = c.verdict === "blocked";
    ledger.recordBenchResult({
      candidateId: cand.id,
      phase: "bench",
      samples: around(c.value),
      stats: stat(c.value),
      guardrails: blocked ? [{ name: "protected paths", passed: false, detail: c.reason }] : [{ name: "pnpm test:unit", passed: true }, { name: "error_rate < 0.001", passed: true }],
      startedAt: at + 2e4,
      finishedAt: at + 4 * MIN$1
    });
    if (c.confirm !== void 0) {
      ledger.recordBenchResult({ candidateId: cand.id, phase: "confirm", samples: around(c.confirm), stats: stat(c.confirm), guardrails: [], startedAt: at + 4 * MIN$1, finishedAt: at + 7 * MIN$1 });
    }
    ledger.setVerdict(cand.id, c.verdict, [c.reason], at + 7 * MIN$1);
    if (c.verdict === "verified") {
      ledger.moveRatchet(spec.id, cand.id, c.value, at + 7 * MIN$1);
      best = c.value;
    }
    ledger.appendAgentEvent({ agentId, type: "verdict", payload: { text: `${c.verdict}: ${c.reason}`, data: { sha, verdict: c.verdict, value: c.value, best } }, at: at + 7 * MIN$1 });
    for (const p of parts) ledger.recordSpend({ optimizationId: spec.id, agentId, category: p.cat, usd: Number((p.w * scale).toFixed(4)), source: "seed", at: at + 5 * MIN$1 });
  }
  if (spec.status === "completed") {
    for (const id2 of agentIds) ledger.setAgentStatus(id2, "done");
    ledger.setOptimizationStatus(spec.id, "completed", now - (spec.endedAgo ?? 0) * MIN$1);
  }
}
function hash(s) {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = Math.imul(31, h) + s.charCodeAt(i) | 0;
  return h;
}
function createEngineForApp(ledger, mode, opts = {}) {
  const engine = new DevEngine(ledger, { mode, ...opts.tickMs !== void 0 && { tickMs: opts.tickMs }, ...opts.seed !== void 0 && { seed: opts.seed } });
  if ((opts.seedDemo ?? true) && isLedgerEmpty(ledger)) seedDemo(ledger);
  for (const o of ledger.listOptimizations({ status: ["baselining", "running"] })) void engine.start(o.id);
  return { kind: "dev", engine, sandbox: new DevSandboxProvider(), dispose: () => engine.dispose() };
}
const id = zod.z.string().min(1).max(200);
const agentKind = zod.z.enum(["claude-code", "codex", "mock"]);
const ARGS = {
  listOptimizations: zod.z.tuple([]),
  getOptimization: zod.z.tuple([id]),
  ratchetHistory: zod.z.tuple([id]),
  candidatesWithResults: zod.z.tuple([id]),
  spendBreakdown: zod.z.tuple([id]),
  recentActivity: zod.z.tuple([zod.z.object({ optimizationId: id.optional(), limit: zod.z.number().int().min(1).max(200).optional() }).strict().optional()]),
  agentEvents: zod.z.tuple([id, zod.z.number().int().min(0).optional(), zod.z.number().int().min(1).max(1e3).optional()]),
  validateConfig: zod.z.tuple([zod.z.string().max(2e5)]),
  preflight: zod.z.tuple([zod.z.object({ agents: zod.z.array(agentKind).max(8), configYaml: zod.z.string().max(2e5) }).strict()]),
  sandboxInfo: zod.z.tuple([]),
  getSettings: zod.z.tuple([]),
  createOptimization: zod.z.tuple([
    zod.z.object({
      name: zod.z.string().trim().min(1).max(120).optional(),
      repoUrl: zod.z.string().trim().min(1).max(500),
      baseRef: zod.z.string().trim().min(1).max(200),
      configPath: zod.z.string().trim().min(1).max(300).optional(),
      configYaml: zod.z.string().max(2e5).optional(),
      agents: zod.z.array(agentKind).min(1).max(8),
      budgetUsd: zod.z.number().positive().max(1e5),
      timeLimitSeconds: zod.z.number().int().positive().max(14 * 24 * 3600),
      plateauEvals: zod.z.number().int().min(1).max(1e4)
    }).strict()
  ]),
  startOptimization: zod.z.tuple([id]),
  stopOptimization: zod.z.tuple([id, zod.z.string().max(500).optional()]),
  pauseAgent: zod.z.tuple([id]),
  resumeAgent: zod.z.tuple([id]),
  messageAgent: zod.z.tuple([id, zod.z.string().trim().min(1).max(4e3)]),
  openPullRequest: zod.z.tuple([id]),
  teardown: zod.z.tuple([id]),
  exportReport: zod.z.tuple([id]),
  setMode: zod.z.tuple([zod.z.enum(["live", "simulation"])]),
  setAwsProfile: zod.z.tuple([zod.z.string().trim().max(120).regex(/^[A-Za-z0-9_.@:-]*$/, "profile names may only contain letters, digits and _ . @ : -")]),
  setSecret: zod.z.tuple([zod.z.enum(["anthropicApiKey", "openaiApiKey", "githubToken"]), zod.z.string().min(8).max(4096)]),
  clearSecret: zod.z.tuple([zod.z.enum(["anthropicApiKey", "openaiApiKey", "githubToken"])])
};
class IpcValidationError extends Error {
  constructor(method, issues) {
    super(`Invalid arguments for ${method}: ${issues}`);
    this.name = "IpcValidationError";
  }
}
const METHODS = Object.keys(ARGS);
function isTrustedUrl(url, devServerUrl) {
  if (url.startsWith("file://")) return true;
  if (!devServerUrl) return false;
  try {
    return new URL(url).origin === new URL(devServerUrl).origin;
  } catch {
    return false;
  }
}
function registerIpc(ipc, service, opts = {}) {
  for (const method of METHODS) {
    const schema2 = ARGS[method];
    ipc.handle(IPC[method], async (event, ...args) => {
      if (opts.isTrustedSender && !opts.isTrustedSender(event)) throw new Error(`Untrusted sender for ${method}`);
      const parsed = schema2.safeParse(args);
      if (!parsed.success) {
        throw new IpcValidationError(method, parsed.error.issues.map((i) => `${i.path.join(".") || "args"}: ${i.message}`).join("; "));
      }
      const fn = service[method];
      return fn.apply(service, parsed.data);
    });
  }
}
const MIN = 6e4;
function unitForMetric(metric) {
  const m = metric.toLowerCase();
  if (/(^|[._])(rss|mem|memory|heap)/.test(m) || /_mb$/.test(m)) return "MB";
  if (/(throughput|events_per_s|per_s|rps|qps)/.test(m)) return "k/s";
  if (/(cold_start|\.s$|_s$|seconds)/.test(m)) return "s";
  if (/(duration|latency|render|\.p\d+|_ms$|ms)/.test(m)) return "ms";
  return "";
}
function relTime(at, now) {
  const d = Math.max(0, now - at);
  if (d < MIN) return "just now";
  if (d < 60 * MIN) return `${Math.floor(d / MIN)}m ago`;
  if (d < 24 * 60 * MIN) return `${Math.floor(d / (60 * MIN))}h ago`;
  const days = Math.floor(d / (24 * 60 * MIN));
  return days === 1 ? "yesterday" : `${days}d ago`;
}
function toUiStatus(status, prUrl) {
  if (status === "completed" && prUrl) return "pr-open";
  return status;
}
function uiDirection(d) {
  return d === "minimize" ? "lower" : "higher";
}
function median(xs) {
  if (xs.length === 0) return 0;
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}
function downsample(xs, n = 8) {
  if (xs.length <= n) return [...xs];
  return Array.from({ length: n }, (_, i) => xs[Math.round(i * (xs.length - 1) / (n - 1))]);
}
const optName = (o) => o.name ?? `${o.repo} ${o.metric}`;
function toOptimization(s, meta, now) {
  const live = s.status === "running" || s.status === "baselining";
  return {
    id: s.id,
    name: optName(s),
    repo: s.repo,
    metric: s.metric,
    unit: unitForMetric(s.metric),
    direction: uiDirection(s.direction),
    baseline: s.baselineValue,
    best: s.bestValue,
    gainPct: s.gainPct,
    status: toUiStatus(s.status, meta.prUrl(s.id)),
    agents: s.agentKinds,
    spendUsd: s.spendUsd,
    budgetUsd: s.budgetUsd,
    updated: live ? "live" : relTime(s.endedAt ?? s.startedAt ?? s.createdAt, now),
    candidateCount: s.candidateCount,
    verifiedWins: s.verifiedWins,
    createdAt: s.createdAt
  };
}
function buildSeries(ledger, id2, now) {
  const opt = ledger.getOptimization(id2);
  if (!opt) throw new Error(`optimization ${id2} not found`);
  const t0 = opt.startedAt ?? opt.createdAt;
  const minutes = (at) => Math.max(0, (at - t0) / MIN);
  const baseline = ledger.getBaseline(id2);
  const moves = ledger.ratchetHistory(id2);
  const cands = ledger.candidatesWithResults(id2);
  const steps = [];
  if (baseline) steps.push({ t: 0, value: baseline.value });
  for (const m of moves) steps.push({ t: minutes(m.at), value: m.toValue });
  const points = cands.filter((c) => c.benchValue !== null).map((c) => ({
    t: minutes(c.submittedAt),
    value: c.benchValue,
    agent: c.agentKind,
    verified: c.movedRatchet,
    label: `${c.sha.slice(0, 7)} · ${c.message || c.verdict}`
  }));
  const last = Math.max(0, ...steps.map((s) => s.t), ...points.map((p) => p.t));
  const live = opt.status === "running" || opt.status === "baselining";
  return {
    metric: opt.metric,
    unit: unitForMetric(opt.metric),
    direction: uiDirection(opt.direction),
    baseline: baseline?.value ?? null,
    steps,
    points,
    noiseFloor: baseline?.noiseFloor ?? null,
    tMax: live ? Math.max(last, minutes(now)) : last
  };
}
function toCandidateVM(c, opt, baseline, direction) {
  const t0 = opt.startedAt ?? opt.createdAt;
  const bench = c.results.filter((r) => r.phase === "bench").at(-1);
  let vs = null;
  if (baseline && c.benchValue !== null) {
    const delta = direction === "minimize" ? baseline - c.benchValue : c.benchValue - baseline;
    vs = delta / Math.abs(baseline) * 100;
  }
  return {
    id: c.id,
    sha: c.sha.slice(0, 7),
    agentId: c.agentId,
    agent: c.agentKind,
    message: c.message,
    submittedAt: c.submittedAt,
    t: Math.max(0, (c.submittedAt - t0) / MIN),
    verdict: c.verdict,
    reasons: c.reasons,
    benchValue: c.benchValue,
    confirmValue: c.confirmValue,
    vsBaselinePct: vs,
    movedRatchet: c.movedRatchet,
    samples: bench?.samples ?? [],
    stats: bench?.stats ?? {}
  };
}
function buildCandidates(ledger, id2) {
  const opt = ledger.getOptimization(id2);
  if (!opt) throw new Error(`optimization ${id2} not found`);
  const baseline = ledger.getBaseline(id2)?.value ?? null;
  return ledger.candidatesWithResults(id2).map((c) => toCandidateVM(c, opt, baseline, opt.direction));
}
const CATEGORY_LABEL = {
  agent_runner: "Agent runners",
  experiment: "Experiment instances",
  bench_runner: "Bench runner",
  storage_egress: "Storage & egress",
  tokens: "Model tokens"
};
function toSpendVM(b, budgetUsd, series) {
  return {
    total: b.total,
    estimated: b.estimated,
    actual: b.actual,
    budgetUsd,
    byCategory: Object.entries(b.byCategory).map(([id2, usd]) => ({ id: id2, label: CATEGORY_LABEL[id2] ?? id2, usd })).sort((a, b2) => b2.usd - a.usd),
    byAgent: b.byAgent.map((a) => ({ agentId: a.agentId, kind: a.kind, branch: a.branch, usd: a.total })),
    series
  };
}
function spendSeries(ledger, id2, points = 40) {
  const opt = ledger.getOptimization(id2);
  if (!opt) return [];
  const t0 = opt.startedAt ?? opt.createdAt;
  const rows = ledger.db.select({ at: spendEvents.at, usd: spendEvents.usd, opt: spendEvents.optimizationId }).from(spendEvents).all().filter((r) => r.opt === id2).sort((a, b) => a.at - b.at);
  let cum = 0;
  const series = rows.map((r) => ({ t: Math.max(0, (r.at - t0) / MIN), usd: cum += r.usd }));
  if (series.length <= points) return series;
  return Array.from({ length: points }, (_, i) => series[Math.round(i * (series.length - 1) / (points - 1))]);
}
function buildSpend(ledger, id2) {
  const opt = ledger.getOptimization(id2);
  if (!opt) throw new Error(`optimization ${id2} not found`);
  return toSpendVM(ledger.spendBreakdown(id2), opt.budgetUsd, spendSeries(ledger, id2));
}
function toActivity(a, metricUnit, now) {
  const short = a.sha.slice(0, 7);
  if (a.kind === "ratchet_move") {
    const unit = metricUnit(a.optimizationId);
    return {
      id: `m-${a.candidateId}`,
      optimizationId: a.optimizationId,
      agent: a.agentKind,
      text: `Verified win: ${a.message || short} · ${fmtNum(a.fromValue)} → ${fmtNum(a.toValue)}${unit ? ` ${unit}` : ""}`,
      at: relTime(a.at, now),
      atMs: a.at,
      tone: "mint"
    };
  }
  const why = a.reasons[0] ?? "";
  const text2 = {
    verified: `Candidate verified: ${a.message || short}`,
    rejected: `Candidate rejected: ${why || a.message || short}`,
    failed_confirm: `Failed confirm on the hidden trace: ${a.message || short}`,
    blocked: `Blocked: ${why || a.message || short}`
  };
  return {
    id: `v-${a.candidateId}`,
    optimizationId: a.optimizationId,
    agent: a.agentKind,
    text: text2[a.verdict] ?? `${a.verdict}: ${short}`,
    at: relTime(a.at, now),
    atMs: a.at,
    tone: a.verdict === "verified" ? "mint" : a.verdict === "blocked" || a.verdict === "failed_confirm" ? "rose" : "neutral"
  };
}
function buildActivity(ledger, now, limit = 12, optimizationId) {
  const units = new Map(ledger.listOptimizations().map((o) => [o.id, unitForMetric(o.metric)]));
  const items = ledger.recentActivity(limit * 2, optimizationId ? { optimizationId } : {});
  const moved = new Set(items.filter((i) => i.kind === "ratchet_move").map((i) => i.candidateId));
  return items.filter((i) => !(i.kind === "verdict" && i.verdict === "verified" && moved.has(i.candidateId))).slice(0, limit).map((i) => toActivity(i, (id2) => units.get(id2) ?? "", now));
}
function fmtNum(n) {
  return Number.isInteger(n) ? String(n) : String(Number(n.toFixed(2)));
}
function buildKpis(ledger, rows, now) {
  const moves = rows.flatMap((r) => ledger.ratchetHistory(r.id)).sort((a, b) => a.at - b.at);
  const wins = moves.length;
  const weekAgo = now - 7 * 24 * 60 * MIN;
  const winsThisWeek = moves.filter((m) => m.at >= weekAgo).length;
  const gains = rows.map((r) => r.gainPct).filter((g) => g !== null && g > 0);
  const medianGain = median(gains);
  const gainTrend = downsample(moves.map((_, i) => median(moves.slice(0, i + 1).map((m) => m.gainPct))));
  const spend = rows.reduce((a, r) => a + r.spendUsd, 0);
  const ids = new Set(rows.map((r) => r.id));
  const events = ledger.db.select({ at: spendEvents.at, usd: spendEvents.usd, opt: spendEvents.optimizationId }).from(spendEvents).all().filter((e) => ids.has(e.opt)).sort((a, b) => a.at - b.at);
  let cum = 0;
  const spendTrend = downsample(events.map((e) => cum += e.usd));
  const dayStart = now - 24 * 60 * MIN;
  const today = events.filter((e) => e.at >= dayStart).reduce((a, e) => a + e.usd, 0);
  const cpw = wins > 0 ? spend / wins : 0;
  const winTrend = downsample(moves.map((_, i) => i + 1));
  return [
    { id: "wins", label: "Verified wins", value: String(wins), delta: `+${winsThisWeek} this week`, trend: winTrend },
    { id: "gain", label: "Median gain", value: gains.length ? medianGain.toFixed(0) : "—", unit: gains.length ? "%" : "", delta: `${gains.length} optimization${gains.length === 1 ? "" : "s"}`, trend: gainTrend },
    { id: "spend", label: "Spend", value: `$${spend.toFixed(0)}`, delta: `$${today.toFixed(0)} last 24h`, trend: spendTrend },
    { id: "cpw", label: "Cost per win", value: wins ? `$${cpw.toFixed(1)}` : "—", delta: wins ? `${wins} verified win${wins === 1 ? "" : "s"}` : "no wins yet", trend: downsample(moves.map((_, i) => i + 1 === 0 ? 0 : spend / (i + 1))) }
  ];
}
function parsePayload(raw) {
  if (!raw) return { text: "" };
  try {
    const p = JSON.parse(raw);
    if (p && typeof p === "object") {
      const o = p;
      const out = { text: typeof o.text === "string" ? o.text : "" };
      if (typeof o.data?.from === "string") out.from = o.data.from;
      return out;
    }
    return { text: String(p) };
  } catch {
    return { text: raw };
  }
}
function toEventVM$1(r) {
  return { id: r.id, at: r.at, type: r.type, text: parsePayload(r.payload).text };
}
function agentEventsPage(ledger, agentId, cursor, limit = 200) {
  if (!ledger.getAgent(agentId)) throw new Error(`agent ${agentId} not found`);
  const rows = cursor === void 0 ? ledger.sqlite.prepare("SELECT * FROM (SELECT id, agent_id, at, type, payload FROM agent_events WHERE agent_id = ? ORDER BY id DESC LIMIT ?) ORDER BY id ASC").all(agentId, limit) : ledger.sqlite.prepare("SELECT id, agent_id, at, type, payload FROM agent_events WHERE agent_id = ? AND id > ? ORDER BY id ASC LIMIT ?").all(agentId, cursor, limit);
  return { events: rows.map(toEventVM$1), cursor: rows.at(-1)?.id ?? cursor ?? 0 };
}
function latestAgentMessage(ledger, agentId) {
  const rows = ledger.sqlite.prepare("SELECT id, agent_id, at, type, payload FROM agent_events WHERE agent_id = ? AND type = 'message' ORDER BY id DESC LIMIT 20").all(agentId);
  for (const r of rows) {
    const p = parsePayload(r.payload);
    if (p.from !== "user" && p.text) return p.text;
  }
  return null;
}
function lastEventId(ledger, agentId) {
  const r = ledger.sqlite.prepare("SELECT max(id) AS m FROM agent_events WHERE agent_id = ?").get(agentId);
  return r.m ?? 0;
}
function toAgentVM(ledger, a, cands, spendUsd) {
  const mine = cands.filter((c) => c.agentId === a.id);
  const recent = mine.filter((c) => c.benchValue !== null).slice(-10).map((c) => ({ value: c.benchValue, verdict: c.verdict, label: `${c.sha.slice(0, 7)} ${c.verdict}` }));
  return {
    id: a.id,
    kind: a.kind,
    branch: a.branch,
    status: a.status,
    hypothesis: latestAgentMessage(ledger, a.id),
    recent,
    stats: {
      candidates: mine.length,
      verified: mine.filter((c) => c.movedRatchet).length,
      rejected: mine.filter((c) => c.verdict === "rejected" || c.verdict === "failed_confirm").length,
      blocked: mine.filter((c) => c.verdict === "blocked").length,
      spendUsd
    },
    lastEventId: lastEventId(ledger, a.id),
    runnerInstanceId: a.runnerInstanceId
  };
}
function buildPipeline(cands) {
  const latest = cands.at(-1);
  const stage = (id2, label, state, detail) => ({ id: id2, label, state, detail });
  if (!latest) {
    return {
      candidate: null,
      queued: 0,
      stages: [
        stage("checkout", "Check out sha", "idle", "waiting for a candidate"),
        stage("guardrails", "Guardrails", "idle", ""),
        stage("bench", "Bench ×N", "idle", ""),
        stage("confirm", "Confirm on hidden trace", "idle", ""),
        stage("verdict", "Verdict", "idle", "")
      ]
    };
  }
  const bench = latest.results.filter((r) => r.phase === "bench").at(-1);
  const confirm = latest.results.filter((r) => r.phase === "confirm").at(-1);
  const pending = latest.verdict === "pending";
  const guardFailed = bench?.guardrails.some((g) => !g.passed) ?? false;
  const stages = [
    stage("checkout", "Check out sha", "done", latest.sha.slice(0, 7)),
    stage(
      "guardrails",
      "Guardrails",
      !bench ? pending ? "active" : "idle" : guardFailed ? "failed" : "done",
      bench ? guardFailed ? bench.guardrails.filter((g) => !g.passed).map((g) => g.name).join(", ") : `${bench.guardrails.length} passed` : "running…"
    ),
    stage("bench", `Bench ×${bench?.samples.length ?? "N"}`, bench ? "done" : guardFailed ? "idle" : pending ? "active" : "idle", bench ? `median ${fmtNum(bench.stats.value)}` : "queued"),
    stage(
      "confirm",
      "Confirm on hidden trace",
      confirm ? latest.verdict === "failed_confirm" ? "failed" : "done" : bench && pending && !guardFailed ? "active" : "idle",
      confirm ? `median ${fmtNum(confirm.stats.value)}` : pending ? "running…" : "skipped"
    ),
    stage("verdict", "Verdict", pending ? "active" : latest.verdict === "verified" ? "done" : "failed", pending ? "deciding…" : latest.verdict.replace("_", " "))
  ];
  return { candidate: { id: latest.id, sha: latest.sha.slice(0, 7), agent: latest.agentKind }, queued: Math.max(0, cands.filter((c) => c.verdict === "pending").length - 1), stages };
}
function buildTeardown(status, agentCount, tornDown) {
  const stopped = ["completed", "stopped", "failed"].includes(status) && agentCount >= 0;
  return [
    { id: "agents", label: "Agents stopped", done: stopped },
    { id: "identities", label: "Agent IAM identities revoked and deleted", done: stopped },
    { id: "instances", label: "Runner, experiment and bench instances terminated", done: stopped },
    { id: "secrets", label: "Per-agent secrets deleted from the sandbox", done: tornDown },
    { id: "sandbox", label: "Sandbox resources destroyed (tofu destroy)", done: tornDown }
  ];
}
function buildDetail(ledger, id2, meta, now) {
  const summary = ledger.listOptimizations().find((o) => o.id === id2);
  const opt = ledger.getOptimization(id2);
  if (!summary || !opt) throw new Error(`optimization ${id2} not found`);
  const baseline = ledger.getBaseline(id2);
  const cands = ledger.candidatesWithResults(id2);
  const spend = ledger.spendBreakdown(id2);
  const agentSpend = new Map(spend.byAgent.map((a) => [a.agentId, a.total]));
  const agents2 = ledger.listAgents(id2).map((a) => toAgentVM(ledger, a, cands, agentSpend.get(a.id) ?? 0));
  const start = opt.startedAt;
  const elapsed = start ? Math.max(0, ((opt.endedAt ?? now) - start) / 1e3) : 0;
  return {
    summary: toOptimization(summary, meta, now),
    baseRef: opt.baseRef,
    minGain: opt.minGain,
    timeLimitS: opt.timeLimitS,
    startedAt: opt.startedAt,
    endedAt: opt.endedAt,
    elapsedS: elapsed,
    baselineSamples: baseline?.samples ?? [],
    baselineStats: baseline?.stats ?? {},
    noiseFloor: baseline?.noiseFloor ?? null,
    agents: agents2,
    pipeline: buildPipeline(cands),
    prUrl: meta.prUrl(id2),
    teardown: buildTeardown(opt.status, agents2.length, meta.teardownDone(id2)),
    configYaml: opt.configYaml
  };
}
function validateConfigText(text2) {
  const r = parseConfig(text2);
  if (!r.ok) return { ok: false, errors: r.errors };
  const c = r.config;
  const summary = {
    metric: c.objective.metric,
    direction: c.objective.direction,
    minGain: c.objective.min_gain,
    benchRun: c.bench.run,
    repeats: c.bench.repeats,
    hardware: c.bench.hardware,
    guardrails: c.guardrails.map((g) => g.kind === "command" ? `run: ${g.run}` : `${g.metric} ${g.raw}`),
    protectedGlobs: c.protected,
    instanceTypes: c.agent.allowed_instance_types
  };
  return { ok: true, summary };
}
function buildReport(ledger, id2, meta, now) {
  const d = buildDetail(ledger, id2, meta, now);
  const s = d.summary;
  const series = buildSeries(ledger, id2, now);
  const cands = buildCandidates(ledger, id2);
  const spend = buildSpend(ledger, id2);
  const u = s.unit ? ` ${s.unit}` : "";
  const n = (v) => v === null ? "—" : `${fmtNum(v)}${u}`;
  const lines = [
    `# Ratchet report: ${s.name}`,
    "",
    `Repository: ${s.repo} (${d.baseRef})  `,
    `Metric: ${s.metric} (${series.direction} is better)  `,
    `Status: ${s.status}`,
    "",
    "## Result",
    "",
    `**${n(s.baseline)} → ${n(s.best)}**${s.gainPct !== null ? ` (${s.gainPct.toFixed(1)}% better)` : ""} with ${s.verifiedWins} verified win${s.verifiedWins === 1 ? "" : "s"} out of ${s.candidateCount} candidates.`,
    "",
    "## Verified wins",
    "",
    ...cands.filter((c) => c.movedRatchet).length ? cands.filter((c) => c.movedRatchet).map((c) => `- \`${c.sha}\` (${c.agent}) ${c.message}: ${n(c.benchValue)}, hidden-trace ${n(c.confirmValue)}`) : ["_None yet._"],
    "",
    "## Spend",
    "",
    `Total $${spend.total.toFixed(2)} of $${spend.budgetUsd.toFixed(2)} budget (${spend.estimated > 0 ? "includes estimates" : "reconciled"}).`,
    "",
    ...spend.byCategory.filter((c) => c.usd > 0).map((c) => `- ${c.label}: $${c.usd.toFixed(2)}`),
    "",
    "## All candidates",
    "",
    "| sha | agent | verdict | bench | vs baseline |",
    "|---|---|---|---|---|",
    ...cands.map((c) => `| ${c.sha} | ${c.agent} | ${c.verdict} | ${n(c.benchValue)} | ${c.vsBaselinePct === null ? "—" : `${c.vsBaselinePct.toFixed(1)}%`} |`),
    ""
  ];
  return lines.join("\n");
}
const SANDBOX_ID = "ratchet-sandbox";
const DEFAULT_INSTANCE_TYPES = ["c7i.2xlarge", "c7i.xlarge", "m7i.large"];
function createService(deps) {
  const { ledger, settings } = deps;
  const now = deps.now ?? Date.now;
  const listeners = /* @__PURE__ */ new Set();
  let app = deps.createApp(settings.mode);
  let unsubscribe = wire(app);
  const meta = { prUrl: (id2) => settings.prUrl(id2), teardownDone: (id2) => settings.tornDown(id2) };
  function wire(a) {
    return a.engine.subscribe((e) => {
      const vm = toEventVM(e);
      for (const l of listeners) l(vm);
    });
  }
  const engineInfo = () => ({ kind: app.kind, mode: app.engine.mode() });
  const settingsVM = () => ({
    mode: settings.mode,
    awsProfile: settings.awsProfile,
    secrets: {
      anthropicApiKey: settings.hasSecret("anthropicApiKey"),
      openaiApiKey: settings.hasSecret("openaiApiKey"),
      githubToken: settings.hasSecret("githubToken")
    },
    encryptionAvailable: settings.encryptionAvailable,
    engine: engineInfo()
  });
  const sandboxInput = (types = DEFAULT_INSTANCE_TYPES) => ({
    sandboxId: SANDBOX_ID,
    region: "us-east-1",
    allowedInstanceTypes: types,
    maxInstancesPerAgent: 2,
    maxInstanceLifetimeHours: 4,
    tags: { "ratchet:managed": "true" }
  });
  return {
    async listOptimizations() {
      const t = now();
      const rows = ledger.listOptimizations();
      const optimizations2 = rows.map((r) => toOptimization(r, meta, t));
      const featured = rows.find((r) => r.status === "running" || r.status === "baselining") ?? rows.find((r) => r.baselineValue !== null) ?? null;
      return {
        kpis: buildKpis(ledger, rows, t),
        featuredId: featured?.id ?? null,
        featuredSeries: featured ? buildSeries(ledger, featured.id, t) : null,
        optimizations: optimizations2,
        activity: buildActivity(ledger, t, 8),
        engine: engineInfo()
      };
    },
    async getOptimization(id2) {
      return buildDetail(ledger, id2, meta, now());
    },
    async ratchetHistory(id2) {
      return buildSeries(ledger, id2, now());
    },
    async candidatesWithResults(id2) {
      return buildCandidates(ledger, id2);
    },
    async spendBreakdown(id2) {
      return buildSpend(ledger, id2);
    },
    async recentActivity(opts) {
      return buildActivity(ledger, now(), opts?.limit ?? 20, opts?.optimizationId);
    },
    async agentEvents(agentId, cursor, limit) {
      return agentEventsPage(ledger, agentId, cursor, limit);
    },
    async validateConfig(text2) {
      return validateConfigText(text2);
    },
    async preflight({ agents: agents2, configYaml }) {
      return preflight(agents2, configYaml);
    },
    async sandboxInfo() {
      const latest = ledger.listOptimizations()[0];
      const cfg = latest ? validateConfigText(ledger.getOptimization(latest.id).configYaml) : null;
      const types = cfg?.ok ? cfg.summary.instanceTypes : DEFAULT_INSTANCE_TYPES;
      const input = sandboxInput(types);
      const [plan, outputs] = await Promise.all([app.sandbox.plan(input), app.sandbox.outputs(SANDBOX_ID)]);
      const accountId = /arn:aws:[^:]*::(\d{12}):/.exec(outputs?.evalIdentityRef ?? "")?.[1] ?? null;
      return {
        plan,
        outputs,
        account: { sandboxId: SANDBOX_ID, region: input.region, profile: settings.awsProfile || "default", accountId, simulated: app.kind === "dev" || settings.mode === "simulation" },
        limits: { allowedInstanceTypes: input.allowedInstanceTypes, maxInstancesPerAgent: input.maxInstancesPerAgent, maxInstanceLifetimeHours: input.maxInstanceLifetimeHours }
      };
    },
    async getSettings() {
      return settingsVM();
    },
    async createOptimization(input) {
      if (input.configYaml !== void 0) {
        const v = validateConfigText(input.configYaml);
        if (!v.ok) throw new Error(`invalid ratchet.yaml: ${v.errors.map((e) => e.path ? `${e.path}: ${e.message}` : e.message).join("; ")}`);
      }
      return app.engine.createOptimization(input);
    },
    async startOptimization(id2) {
      await app.engine.start(id2);
    },
    async stopOptimization(id2, reason) {
      await app.engine.stop(id2, reason);
    },
    async pauseAgent(agentId) {
      await app.engine.pauseAgent(agentId);
    },
    async resumeAgent(agentId) {
      await app.engine.resumeAgent(agentId);
    },
    async messageAgent(agentId, text2) {
      await app.engine.messageAgent(agentId, text2);
    },
    async openPullRequest(id2) {
      const pr = await app.engine.openPullRequest(id2);
      settings.setPrUrl(id2, pr.url);
      return pr;
    },
    async teardown(id2) {
      await app.engine.teardown(id2);
      settings.markTornDown(id2);
    },
    async exportReport(id2) {
      const md = buildReport(ledger, id2, meta, now());
      const name = `ratchet-report-${id2}.md`;
      const path = deps.saveReport ? await deps.saveReport(name, md) : null;
      return path ? { saved: true, path } : { saved: false };
    },
    async setMode(mode) {
      if (mode !== settings.mode) {
        settings.setMode(mode);
        unsubscribe();
        app.dispose();
        app = deps.createApp(mode);
        unsubscribe = wire(app);
      }
      return settingsVM();
    },
    async setAwsProfile(profile) {
      settings.setAwsProfile(profile);
      return settingsVM();
    },
    async setSecret(name, value) {
      settings.setSecret(name, value);
      return settingsVM();
    },
    async clearSecret(name) {
      settings.clearSecret(name);
      return settingsVM();
    },
    onEvent(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    dispose() {
      unsubscribe();
      app.dispose();
      listeners.clear();
    }
  };
  async function preflight(agents2, configYaml) {
    const live = settings.mode === "live";
    const miss = (ok, needed) => ok ? "ok" : needed ? "fail" : "warn";
    const cfg = validateConfigText(configYaml);
    const items = [
      {
        id: "config",
        label: "ratchet.yaml is valid",
        state: cfg.ok ? "ok" : "fail",
        detail: cfg.ok ? `${cfg.summary.metric} (${cfg.summary.direction})` : `${cfg.errors.length} error${cfg.errors.length === 1 ? "" : "s"}`
      },
      { id: "agents", label: "At least one agent selected", state: agents2.length > 0 ? "ok" : "fail", detail: agents2.join(", ") || "none" },
      {
        id: "engine",
        label: live ? "Live mode" : "Simulation mode",
        state: app.kind === "dev" ? "warn" : "ok",
        detail: app.kind === "dev" ? "dev engine stub: results are simulated" : live ? "real AWS resources will be created" : "no cloud resources are used"
      }
    ];
    if (agents2.includes("claude-code")) {
      const ok = settings.hasSecret("anthropicApiKey");
      items.push({ id: "anthropic", label: "Anthropic API key", state: miss(ok, live), detail: ok ? "configured" : live ? "required in live mode (Settings)" : "not needed in simulation" });
    }
    if (agents2.includes("codex")) {
      const ok = settings.hasSecret("openaiApiKey");
      items.push({ id: "openai", label: "OpenAI API key", state: miss(ok, live), detail: ok ? "configured" : live ? "required in live mode (Settings)" : "not needed in simulation" });
    }
    {
      const ok = settings.hasSecret("githubToken");
      items.push({ id: "github", label: "GitHub token", state: miss(ok, live), detail: ok ? "configured" : live ? "required to push branches and open the PR" : "not needed in simulation" });
    }
    {
      const ok = settings.awsProfile !== "";
      items.push({ id: "aws", label: "AWS profile", state: miss(ok, live), detail: ok ? settings.awsProfile : live ? "required in live mode (Settings)" : "not needed in simulation" });
    }
    const outputs = await app.sandbox.outputs(SANDBOX_ID);
    items.push({ id: "sandbox", label: "Sandbox applied", state: outputs ? "ok" : "warn", detail: outputs ? outputs.artifactsBucket : "not applied yet; review the plan under AWS sandbox" });
    return items;
  }
}
function toEventVM(e) {
  switch (e.type) {
    case "agent-event":
      return { type: "agent-event", optimizationId: e.optimizationId, agentId: e.agentId };
    case "candidate":
      return { type: "candidate", optimizationId: e.optimizationId, candidateId: e.candidateId, verdict: e.verdict };
    default:
      return e;
  }
}
const DEFAULTS = { mode: "simulation", awsProfile: "", pullRequests: {}, tornDown: {}, secrets: {} };
const ENV_FOR = {
  anthropicApiKey: ["ANTHROPIC_API_KEY"],
  openaiApiKey: ["OPENAI_API_KEY"],
  githubToken: ["GITHUB_TOKEN", "GH_TOKEN"]
};
class SettingsStore {
  constructor(file, cipher, env = process.env) {
    this.file = file;
    this.cipher = cipher;
    this.env = env;
    this.data = { ...DEFAULTS, pullRequests: {}, tornDown: {}, secrets: {} };
    if (file && fs.existsSync(file)) {
      try {
        const raw = JSON.parse(fs.readFileSync(file, "utf8"));
        this.data = { ...this.data, ...raw, secrets: { ...raw.secrets }, pullRequests: { ...raw.pullRequests }, tornDown: { ...raw.tornDown } };
      } catch {
      }
    }
  }
  file;
  cipher;
  env;
  data;
  get mode() {
    return this.data.mode;
  }
  get awsProfile() {
    return this.data.awsProfile;
  }
  get encryptionAvailable() {
    return this.cipher.isAvailable();
  }
  setMode(mode) {
    this.data.mode = mode;
    this.save();
  }
  setAwsProfile(profile) {
    this.data.awsProfile = profile;
    this.save();
  }
  prUrl(id2) {
    return this.data.pullRequests[id2] ?? null;
  }
  setPrUrl(id2, url) {
    this.data.pullRequests[id2] = url;
    this.save();
  }
  tornDown(id2) {
    return this.data.tornDown[id2] === true;
  }
  markTornDown(id2) {
    this.data.tornDown[id2] = true;
    this.save();
  }
  setSecret(name, value) {
    if (!this.cipher.isAvailable()) throw new Error("OS keychain encryption is unavailable; refusing to store the secret in plaintext");
    this.data.secrets[name] = this.cipher.encrypt(value).toString("base64");
    this.save();
  }
  clearSecret(name) {
    delete this.data.secrets[name];
    this.save();
  }
  /** Configured = stored in the keychain, or provided through the environment. */
  hasSecret(name) {
    return this.data.secrets[name] !== void 0 || ENV_FOR[name].some((k) => !!this.env[k]);
  }
  /** Main-process only: hand to the engine, never to the renderer. */
  getSecret(name) {
    const enc = this.data.secrets[name];
    if (enc !== void 0 && this.cipher.isAvailable()) return this.cipher.decrypt(Buffer.from(enc, "base64"));
    for (const k of ENV_FOR[name]) if (this.env[k]) return this.env[k];
    return null;
  }
  save() {
    if (!this.file) return;
    fs.mkdirSync(node_path.dirname(this.file), { recursive: true });
    const tmp = `${this.file}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(this.data, null, 2), { mode: 384 });
    fs.renameSync(tmp, this.file);
  }
}
let mainWindow = null;
function createWindow() {
  const win = new electron.BrowserWindow({
    width: 1360,
    height: 880,
    minWidth: 1100,
    minHeight: 700,
    backgroundColor: "#0B1019",
    titleBarStyle: process.platform === "darwin" ? "hiddenInset" : "default",
    webPreferences: {
      preload: node_path.join(__dirname, "../preload/index.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true
    }
  });
  mainWindow = win;
  win.on("closed", () => {
    if (mainWindow === win) mainWindow = null;
  });
  win.webContents.on("will-navigate", (e, url) => {
    if (url !== win.webContents.getURL()) e.preventDefault();
  });
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https:\/\//.test(url)) void electron.shell.openExternal(url);
    return { action: "deny" };
  });
  if (process.env["ELECTRON_RENDERER_URL"]) {
    void win.loadURL(process.env["ELECTRON_RENDERER_URL"]);
  } else {
    void win.loadFile(node_path.join(__dirname, "../renderer/index.html"));
  }
}
function migrationsFolder() {
  return electron.app.isPackaged ? node_path.join(process.resourcesPath, "migrations") : node_path.join(__dirname, "../../../core/src/ledger/migrations");
}
electron.app.whenReady().then(() => {
  const userData = electron.app.getPath("userData");
  fs.mkdirSync(userData, { recursive: true });
  const ledger = openLedger(node_path.join(userData, "ratchet.db"), { migrationsFolder: migrationsFolder() });
  const settings = new SettingsStore(node_path.join(userData, "settings.json"), {
    isAvailable: () => electron.safeStorage.isEncryptionAvailable(),
    encrypt: (s) => electron.safeStorage.encryptString(s),
    decrypt: (b) => electron.safeStorage.decryptString(b)
  });
  const service = createService({
    ledger,
    settings,
    createApp: (mode) => createEngineForApp(ledger, mode),
    saveReport: async (defaultName, markdown) => {
      const res = await electron.dialog.showSaveDialog({ defaultPath: defaultName, filters: [{ name: "Markdown", extensions: ["md"] }] });
      if (res.canceled || !res.filePath) return null;
      fs.writeFileSync(res.filePath, markdown, "utf8");
      return res.filePath;
    }
  });
  registerIpc(electron.ipcMain, service, {
    isTrustedSender: (event) => {
      const url = event.senderFrame?.url ?? "";
      return isTrustedUrl(url, process.env["ELECTRON_RENDERER_URL"]);
    }
  });
  service.onEvent((e) => mainWindow?.webContents.send(IPC.event, e));
  electron.app.on("before-quit", () => {
    service.dispose();
    ledger.close();
  });
  createWindow();
  electron.app.on("activate", () => {
    if (electron.BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});
electron.app.on("window-all-closed", () => {
  if (process.platform !== "darwin") electron.app.quit();
});
