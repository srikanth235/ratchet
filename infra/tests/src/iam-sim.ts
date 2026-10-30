import type { Condition, PolicyDocument, Statement } from "./render.js";

/**
 * A deliberately small IAM evaluator, enough to check the intent of the agent
 * boundary without AWS. It evaluates ONE policy document (a permission
 * boundary, or a resource policy): explicit deny > allow > implicit deny.
 * Features it does not model throw rather than being silently ignored.
 */

export type Decision = "Allow" | "ExplicitDeny" | "ImplicitDeny";

export interface Request {
  action: string;
  resource: string;
  /** Lower- or mixed-case keys; lookups are case-insensitive. */
  context?: Record<string, string>;
}

const ctxGet = (ctx: Record<string, string>, key: string): string | undefined => {
  const lower = key.toLowerCase();
  for (const [k, v] of Object.entries(ctx)) if (k.toLowerCase() === lower) return v;
  return undefined;
};

const toList = <T>(v: T | T[] | undefined): T[] => (v === undefined ? [] : Array.isArray(v) ? v : [v]);

function glob(pattern: string, value: string, caseInsensitive = false): boolean {
  const re = new RegExp(
    "^" +
      pattern
        .replace(/[.+^${}()|[\]\\]/g, "\\$&")
        .replace(/\*/g, ".*")
        .replace(/\?/g, ".") +
      "$",
    caseInsensitive ? "si" : "s",
  );
  return re.test(value);
}

/** Substitutes ${aws:PrincipalTag/...} style variables; undefined if one is unresolved. */
function substitute(value: string, ctx: Record<string, string>): string | undefined {
  let unresolved = false;
  const out = value.replace(/\$\{([^}]+)\}/g, (_m, key: string) => {
    const v = ctxGet(ctx, key);
    if (v === undefined) unresolved = true;
    return v ?? "";
  });
  return unresolved ? undefined : out;
}

function conditionHolds(condition: Condition | undefined, ctx: Record<string, string>): boolean {
  if (!condition) return true;
  for (const [operator, keys] of Object.entries(condition)) {
    if (/^(ForAnyValue|ForAllValues):|IfExists$/.test(operator)) {
      throw new Error(`unsupported condition operator: ${operator}`);
    }
    for (const [key, expected] of Object.entries(keys)) {
      const actual = ctxGet(ctx, key);
      const values = toList(expected)
        .map((v) => substitute(v, ctx))
        .filter((v): v is string => v !== undefined);
      let ok: boolean;
      switch (operator) {
        case "Null":
          ok = values.some((v) => (v === "true") === (actual === undefined));
          break;
        case "Bool":
          ok = actual !== undefined && values.some((v) => v === actual);
          break;
        case "StringEquals":
        case "ArnEquals":
          ok = actual !== undefined && values.some((v) => v === actual);
          break;
        case "StringNotEquals":
        case "ArnNotEquals":
          ok = actual === undefined || values.every((v) => v !== actual);
          break;
        case "StringLike":
        case "ArnLike":
          ok = actual !== undefined && values.some((v) => glob(v, actual));
          break;
        case "StringNotLike":
        case "ArnNotLike":
          ok = actual === undefined || values.every((v) => !glob(v, actual));
          break;
        default:
          throw new Error(`unsupported condition operator: ${operator}`);
      }
      if (!ok) return false;
    }
  }
  return true;
}

function matches(s: Statement, req: Request): boolean {
  if (s.NotAction !== undefined || s.NotResource !== undefined) {
    throw new Error("NotAction/NotResource are not supported (and are discouraged in a boundary)");
  }
  const ctx = req.context ?? {};
  const actionOk = toList(s.Action).some((a) => glob(a, req.action, true));
  if (!actionOk) return false;
  const resourceOk = toList(s.Resource).some((r) => {
    const resolved = substitute(r, ctx);
    return resolved !== undefined && glob(resolved, req.resource);
  });
  if (!resourceOk) return false;
  return conditionHolds(s.Condition, ctx);
}

export function evaluate(policy: PolicyDocument, req: Request): Decision {
  const hits = policy.Statement.filter((s) => matches(s, req));
  if (hits.some((s) => s.Effect === "Deny")) return "ExplicitDeny";
  if (hits.some((s) => s.Effect === "Allow")) return "Allow";
  return "ImplicitDeny";
}
