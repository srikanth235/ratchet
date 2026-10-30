import { z } from 'zod';
import { type InvokeMethod } from './channels.js';
import { SETTING_LABELS, validateLiveSetting, validateSecretFormat, type LiveSettingKey } from './live-settings.js';

// ------------------------------------------------------------------ input schemas

const id = z.string().min(1).max(200);
const agentKind = z.enum(['claude-code', 'codex', 'mock']);
const secretName = z.enum(['anthropicApiKey', 'openaiApiKey', 'githubToken']);

/** A live setting: a string that passes the same format check the Settings screen shows while typing. */
const liveField = (key: LiveSettingKey) =>
  z.string().max(600).superRefine((v, ctx) => {
    const problem = validateLiveSetting(key, v);
    if (problem) ctx.addIssue({ code: 'custom', message: `${SETTING_LABELS[key]}: ${problem}` });
  });

/** Argument tuples per method, validated in the main process before anything is executed. */
export const ARGS = {
  listOptimizations: z.tuple([z.object({ range: z.enum(['7d', '30d', '90d']).optional() }).strict().optional()]),
  getOptimization: z.tuple([id]),
  ratchetHistory: z.tuple([id, z.string().min(1).max(300).optional()]),
  winningChange: z.tuple([id]),
  candidatesWithResults: z.tuple([id]),
  spendBreakdown: z.tuple([id]),
  recentActivity: z.tuple([z.object({ optimizationId: id.optional(), limit: z.number().int().min(1).max(200).optional() }).strict().optional()]),
  agentEvents: z.tuple([id, z.number().int().min(0).optional(), z.number().int().min(1).max(1000).optional()]),
  validateConfig: z.tuple([z.string().max(200_000)]),
  preflight: z.tuple([z.object({ agents: z.array(agentKind).max(8), configYaml: z.string().max(200_000) }).strict()]),
  sandboxInfo: z.tuple([]),
  getSettings: z.tuple([]),
  planSandbox: z.tuple([]),
  applySandbox: z.tuple([]),
  destroySandbox: z.tuple([]),
  createOptimization: z.tuple([
    z
      .object({
        name: z.string().trim().min(1).max(120).optional(),
        repoUrl: z.string().trim().min(1).max(500),
        baseRef: z.string().trim().min(1).max(200),
        configPath: z.string().trim().min(1).max(300).optional(),
        configYaml: z.string().max(200_000).optional(),
        agents: z.array(agentKind).min(1).max(8),
        budgetUsd: z.number().positive().max(100_000),
        timeLimitSeconds: z.number().int().positive().max(14 * 24 * 3600),
        plateauEvals: z.number().int().min(1).max(10_000),
      })
      .strict(),
  ]),
  startOptimization: z.tuple([id]),
  stopOptimization: z.tuple([id, z.string().max(500).optional()]),
  pauseAgent: z.tuple([id]),
  resumeAgent: z.tuple([id]),
  messageAgent: z.tuple([id, z.string().trim().min(1).max(4000)]),
  openPullRequest: z.tuple([id]),
  teardown: z.tuple([id]),
  exportReport: z.tuple([id]),
  setMode: z.tuple([z.enum(['live', 'simulation'])]),
  setAwsProfile: z.tuple([z.string().trim().max(120).regex(/^[A-Za-z0-9_.@:-]*$/, 'profile names may only contain letters, digits and _ . @ : -')]),
  setSecret: z.tuple([
    secretName,
    z.string().min(8).max(4096),
  ]).superRefine(([name, value], ctx) => {
    const problem = validateSecretFormat(name, value);
    if (problem) ctx.addIssue({ code: 'custom', path: [1], message: problem });
  }),
  clearSecret: z.tuple([secretName]),
  saveLiveSettings: z.tuple([
    z
      .object({
        awsProfile: liveField('awsProfile'),
        awsRegion: liveField('awsRegion'),
        awsAuth: z.enum(['profile', 'sso']),
        githubRepo: liveField('githubRepo'),
        runnerImageRef: liveField('runnerImageRef'),
        benchImageRef: liveField('benchImageRef'),
        tofuBinary: liveField('tofuBinary'),
        moduleDir: liveField('moduleDir'),
      })
      .partial()
      .strict(),
  ]),
  testConnection: z.tuple([z.enum(['aws', 'github', 'anthropic', 'openai', 'tofu'])]),
} as const satisfies Record<InvokeMethod, z.ZodType>;
