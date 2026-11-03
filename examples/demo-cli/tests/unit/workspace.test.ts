import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { GOLDEN_SPEC, buildWorkspace, generateTasks, type Workspace } from '../../bench/workspace.js';
import { discoverConfig } from '../../src/config.js';
import { scanPlugins } from '../../src/plugins.js';
import { loadStore } from '../../src/store.js';
import { detectTerminal } from '../../src/terminal.js';
import { ValidationError, validateTask } from '../../src/validate.js';

let ws: Workspace;
beforeAll(() => {
  ws = buildWorkspace(GOLDEN_SPEC);
});
afterAll(() => ws.cleanup());

describe('discoverConfig', () => {
  it('cascades from the nearest file and stops at "root": true', () => {
    const d = discoverConfig(ws.cwd);
    expect(d.sources.map((s) => s.file.slice(ws.root.length + 1))).toEqual(['packages/web/package.json', '.taskrrc.json']);
    expect(d.rootDir).toBe(ws.root);
    expect(d.config.tz).toBe('Europe/Berlin');
    expect(d.config.pageSize).toBe(12);
    expect(d.config.dataFile).toBe(join(ws.root, 'data', 'tasks.json'));
  });

  it('uses defaults when there is no config anywhere', () => {
    const dir = mkdtempSync(join(tmpdir(), 'taskr-empty-'));
    try {
      const d = discoverConfig(dir);
      expect(d.sources).toEqual([]);
      expect(d.config).toMatchObject({ tz: 'UTC', pageSize: 20, dataFile: join(dir, 'tasks.json') });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('ignores unreadable files and package.json without a taskr key', () => {
    const dir = mkdtempSync(join(tmpdir(), 'taskr-bad-'));
    try {
      mkdirSync(join(dir, 'a'));
      writeFileSync(join(dir, '.taskrrc.json'), '{ not json');
      writeFileSync(join(dir, 'a', 'package.json'), JSON.stringify({ name: 'x' }));
      expect(discoverConfig(join(dir, 'a')).sources).toEqual([]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe('scanPlugins', () => {
  it('finds only taskr plugins, sorted, in every node_modules above', () => {
    expect(scanPlugins(ws.cwd)).toEqual([
      { name: '@taskr/plugin-burndown', version: '1.2.0' },
      { name: 'taskr-plugin-gantt', version: '0.4.1' },
    ]);
  });
});

describe('detectTerminal', () => {
  it('returns positive integers', () => {
    const t = detectTerminal();
    expect(Number.isInteger(t.columns) && t.columns > 0).toBe(true);
    expect(Number.isInteger(t.colors) && t.colors > 0).toBe(true);
  });
});

describe('tasks', () => {
  it('generateTasks is deterministic and valid', () => {
    expect(generateTasks(50, 3)).toEqual(generateTasks(50, 3));
    expect(generateTasks(50, 3)).not.toEqual(generateTasks(50, 4));
    generateTasks(300, 3).forEach((t, i) => validateTask(t, i));
  });

  it('validateTask names the offending field', () => {
    const ok = generateTasks(1, 1)[0] as Record<string, unknown>;
    expect(validateTask(ok, 0).id).toBe(1);
    for (const [field, value] of [['id', 0], ['title', ''], ['status', 'later'], ['priority', 5], ['assignee', 'Bob Smith'], ['labels', ['Bad Label']], ['estimate', -1], ['created', '2026-13-40'], ['due', 'soon']] as const) {
      expect(() => validateTask({ ...ok, [field]: value }, 4), field).toThrow(new RegExp(`task #4: ${field} `));
    }
    expect(() => validateTask(null, 0)).toThrow(ValidationError);
  });

  it('loadStore indexes tasks, and a missing file is an empty store', () => {
    const store = loadStore(join(ws.root, 'data', 'tasks.json'));
    expect(store.tasks).toHaveLength(GOLDEN_SPEC.tasks);
    expect(store.byId.get(17)?.id).toBe(17);
    expect([...store.byStatus.values()].reduce((n, l) => n + l.length, 0)).toBe(GOLDEN_SPEC.tasks);
    expect(store.byAssignee.get('maya.chen')!.every((t) => t.assignee === 'maya.chen')).toBe(true);
    expect(store.titleIndex.get('checkout')!.length).toBeGreaterThan(0);
    expect(loadStore(join(ws.root, 'nope.json')).tasks).toEqual([]);
  });
});
