import type { Ledger } from '@ratchet/core';

/**
 * Small key/value table living in the ledger's own SQLite file (same connection, so writes can
 * share a transaction with ledger writes). It holds engine bookkeeping that the ledger schema has
 * no column for: poll cursors, agent identity refs, launch progress, cost watermarks, stop mode.
 * It never holds secret values.
 */
export class KvStore {
  private readonly getStmt;
  private readonly setStmt;
  private readonly delStmt;
  private readonly prefixStmt;

  constructor(ledger: Ledger) {
    const db = ledger.sqlite;
    db.exec('CREATE TABLE IF NOT EXISTS engine_kv (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)');
    this.getStmt = db.prepare('SELECT value FROM engine_kv WHERE key = ?');
    this.setStmt = db.prepare(
      'INSERT INTO engine_kv (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value',
    );
    this.delStmt = db.prepare('DELETE FROM engine_kv WHERE key = ?');
    this.prefixStmt = db.prepare("SELECT key, value FROM engine_kv WHERE key LIKE ? ESCAPE '\\'");
  }

  get(key: string): string | undefined {
    const row = this.getStmt.get(key) as { value: string } | undefined;
    return row?.value;
  }

  set(key: string, value: string): void {
    this.setStmt.run(key, value);
  }

  delete(key: string): void {
    this.delStmt.run(key);
  }

  getNumber(key: string): number | undefined {
    const v = this.get(key);
    if (v === undefined) return undefined;
    const n = Number(v);
    return Number.isFinite(n) ? n : undefined;
  }

  getJson<T>(key: string): T | undefined {
    const v = this.get(key);
    return v === undefined ? undefined : (JSON.parse(v) as T);
  }

  setJson(key: string, value: unknown): void {
    this.set(key, JSON.stringify(value));
  }

  has(key: string): boolean {
    return this.get(key) !== undefined;
  }

  entries(prefix: string): Array<{ key: string; value: string }> {
    const like = prefix.replace(/[\\%_]/g, (c) => `\\${c}`) + '%';
    return this.prefixStmt.all(like) as Array<{ key: string; value: string }>;
  }
}

/** Every kv key the engine writes, in one place. */
export const K = {
  plateau: (opt: string) => `opt:${opt}:plateau_evals`,
  deadline: (opt: string) => `opt:${opt}:deadline_at`,
  warned: (opt: string) => `opt:${opt}:budget_warned`,
  stopMode: (opt: string) => `opt:${opt}:stop_mode`, // 'kill' | 'graceful'
  stopReason: (opt: string) => `opt:${opt}:stop_reason`,
  stopAt: (opt: string) => `opt:${opt}:stop_at`,
  confirmBest: (opt: string) => `opt:${opt}:confirm_best`,
  baseSha: (opt: string) => `opt:${opt}:base_sha`,
  pr: (opt: string) => `opt:${opt}:pr_url`,
  teardown: (opt: string) => `opt:${opt}:teardown_report`,
  cursor: (agent: string) => `agent:${agent}:cursor`,
  identity: (agent: string) => `agent:${agent}:identity`,
  secrets: (agent: string) => `agent:${agent}:secret_refs`,
  launch: (agent: string) => `agent:${agent}:launch_step`,
  revoked: (agent: string) => `agent:${agent}:revoked`,
  acct: (instance: string) => `acct:${instance}`,
  sandboxInput: (sandbox: string) => `sandbox:${sandbox}:input`,
} as const;
