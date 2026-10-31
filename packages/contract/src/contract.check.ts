// Compile-time contract between the desktop app's view models (`packages/desktop/src/shared/types.ts`, the spec the
// daemon was ported from) and the types generated from `api/openapi.yaml` (`./api.d.ts`, from the Rust types).
//
// Nothing runs: `tsc --noEmit` fails when a row below is not `true`, and the error names the row and shows both
// sides. Intentional differences are spelled out in the rows (a `Patch`, a `Within`, an `Extra` key list) and in
// `api/CONTRACT.md`; there is no `any` and no cast anywhere in this file.
//
// To check a new group: add a line to the matching table. `PENDING` lists the groups that are not checked yet.

import type { components, operations } from './api.js';
import type { AssertAll, ByType, Expect, Patch, Same, Sends, Within } from './helpers.js';
import type {
  ActivityItem,
  AgentEventsPage,
  AgentEventVM,
  AgentRecentEval,
  AgentVM,
  AwsProfilesVM,
  AwsProfileVM,
  BenchPipelineVM,
  CandidateVM,
  CompareRow,
  ConfigSummary,
  ConnectionResultVM,
  CreateOptimizationInput,
  DiffFileVM,
  DiffLine,
  EngineEventVM,
  EngineInfo,
  EnvironmentsVM,
  EnvironmentVM,
  GithubFlowVM,
  GithubStatusVM,
  HarnessVM,
  Kpi,
  KpiChange,
  LiveSettingsInput,
  MetricOption,
  Optimization,
  OptimizationDetail,
  OptimizationsSnapshot,
  PipelineStage,
  PreflightInput,
  PreflightItem,
  ProposeTemplateInput,
  ProposeTemplateVM,
  RatchetApi,
  RatchetPoint,
  RatchetSeries,
  RepoInspectVM,
  RepoSourceInput,
  RepoVM,
  SecretName,
  SettingsVM,
  SpanRow,
  SpendLine,
  SpendVM,
  TeardownReceiptVM,
  TemplateVM,
  ValidateConfigResult,
  ValueVM,
  WinningChangeVM,
} from '../../desktop/src/shared/types.js';
import type { OptimizationStatus } from '@ratchet/ui';

type S = components['schemas'];
type Ops = operations;

/** JSON body of the success response of an operation. */
type Success<O extends keyof Ops> = Ops[O]['responses'] extends infer R ? R[Extract<keyof R, 200 | 201 | 202>] : never;
type Reply<O extends keyof Ops> = Success<O> extends { content: { 'application/json': infer B } } ? B : never;
/** JSON request body of an operation. */
type Body<O extends keyof Ops> = Ops[O] extends { requestBody: { content: { 'application/json': infer B } } } ? B : never;
type Query<O extends keyof Ops> = Ops[O]['parameters']['query'] extends infer Q ? NonNullable<Q> : never;

type Resolved<T> = Exclude<T, undefined>;
type Opt<T> = Resolved<T>;

// ---------------------------------------------------------------------------------------------------------------
// Intentional differences, as transformations of the TypeScript side (see api/CONTRACT.md).

/** D1. The engine kind: TypeScript's `dev` (the in-repo stub) is the daemon's `sim`. */
type DevToSim<T> = T extends 'dev' ? 'sim' : T;
type EngineInfoWire = Patch<EngineInfo, { kind: DevToSim<EngineInfo['kind']> }>;

/** D2. `SettingsVM.dev: LiveSettings & {defaults}` is `advanced: {values, defaults}`. */
type SettingsWire = Omit<Patch<SettingsVM, { engine: EngineInfoWire }>, 'dev'> & {
  advanced: { values: Omit<SettingsVM['dev'], 'defaults'>; defaults: SettingsVM['dev']['defaults'] };
};

/** D3. `ValidateConfigResult` is a union keyed on `ok` in TypeScript, one object with both lists on the wire. */
type ValidateWire = {
  ok: boolean;
  summary: Extract<ValidateConfigResult, { ok: true }>['summary'] | null;
  errors: Extract<ValidateConfigResult, { ok: false }>['errors'];
};

/** D1 again: the snapshot's engine is `EngineInfoWire`. */
type SnapshotWire = Patch<OptimizationsSnapshot, { engine: EngineInfoWire }>;

/** D5. The wire carries two template fields the gallery does not show. */
type TemplateExtra = 'defaultValueUsd' | 'example';

// ---------------------------------------------------------------------------------------------------------------
// 1. Schemas: every view model against its generated counterpart. Rust `Foo...Vm` <-> TypeScript `Foo...VM`.

type ViewModelsTable = {
  // optimizations: list, detail
  Kpi: Same<Kpi, S['Kpi']>;
  KpiChange: Same<KpiChange, S['KpiChange']>;
  ValueVM: Same<ValueVM, S['ValueVm']>;
  Optimization: Same<Omit<Optimization, 'status'>, Omit<S['OptimizationVm'], 'status'>>;
  ActivityItem: Same<ActivityItem, S['ActivityVm']>;
  EngineInfo: Same<EngineInfoWire, S['EngineInfo']>;
  OptimizationsSnapshot: Same<Omit<SnapshotWire, 'optimizations'>, Omit<S['OptimizationsSnapshot'], 'optimizations'>>;
  OptimizationsSnapshotRows: Same<Omit<Optimization, 'status'>[], Omit<S['OptimizationsSnapshot']['optimizations'][number], 'status'>[]>;
  MetricOption: Same<MetricOption, S['MetricOption']>;
  RatchetPoint: Same<RatchetPoint, S['RatchetPoint']>;
  RatchetSeries: Same<RatchetSeries, S['RatchetSeries']>;
  AgentRecentEval: Same<AgentRecentEval, S['AgentRecentEval']>;
  AgentVM: Same<AgentVM, S['AgentVm']>;
  PipelineStage: Same<PipelineStage, S['PipelineStage']>;
  BenchPipelineVM: Same<BenchPipelineVM, S['BenchPipelineVm']>;
  TeardownReceiptVM: Same<TeardownReceiptVM, S['TeardownReceiptVm']>;
  CompareRow: Same<CompareRow, S['CompareRow']>;
  SpanRow: Same<SpanRow, S['SpanRow']>;
  OptimizationDetail: Same<Omit<OptimizationDetail, 'summary'>, Omit<S['OptimizationDetail'], 'summary'>>;
  OptimizationDetailSummary: Same<Omit<OptimizationDetail['summary'], 'status'>, Omit<S['OptimizationDetail']['summary'], 'status'>>;
  DiffLine: Same<DiffLine, S['DiffLine']>;
  DiffFileVM: Same<DiffFileVM, S['DiffFileVm']>;
  WinningChangeVM: Same<WinningChangeVM, S['WinningChangeVm']>;
  // candidates, spend, agent log
  CandidateVM: Same<CandidateVM, S['CandidateVm']>;
  SpendLine: Same<SpendLine, S['SpendLine']>;
  SpendVM: Same<SpendVM, S['SpendVm']>;
  AgentEventVM: Same<AgentEventVM, S['AgentEventVm']>;
  AgentEventsPage: Same<AgentEventsPage, S['AgentEventsPage']>;
  // preflight, validation, repos, templates
  ConfigSummary: Same<ConfigSummary, S['ConfigSummary']>;
  ValidateConfigResult: Same<ValidateWire, S['ValidateConfigResult']>;
  PreflightItem: Same<PreflightItem, S['PreflightItem']>;
  RepoVM: Same<RepoVM, S['RepoVm']>;
  RepoInspectVM: Same<Omit<RepoInspectVM, 'templates'>, Omit<S['RepoInspectVm'], 'templates'>>;
  TemplateVM: Same<TemplateVM, Omit<S['TemplateInfo'], TemplateExtra>>;
  RepoInspectTemplates: Same<RepoInspectVM['templates'], Omit<S['RepoInspectVm']['templates'][number], TemplateExtra>[]>;
  ProposeTemplateVM: Same<ProposeTemplateVM, S['ProposeTemplateResult']>;
  // settings, GitHub, AWS, harnesses
  SettingsVM: Same<SettingsWire, S['SettingsVm']>;
  GithubFlowVM: Same<GithubFlowVM, S['GithubFlowVm']>;
  GithubStatusVM: Same<GithubStatusVM, S['GithubStatusVm']>;
  AwsProfileVM: Same<AwsProfileVM, S['AwsProfileVm']>;
  AwsProfilesVM: Same<AwsProfilesVM, S['AwsProfilesVm']>;
  HarnessVM: Same<HarnessVM, S['HarnessVm']>;
  ConnectionResultVM: Same<ConnectionResultVM, S['ConnectionResultVm']>;
  SecretName: Same<SecretName, S['SecretName']>;
  // environments, receipts
  EnvironmentVM: Same<EnvironmentVM, S['EnvironmentVm']>;
  EnvironmentsVM: Same<EnvironmentsVM, S['EnvironmentsVm']>;
};
export type ViewModels = Expect<AssertAll<ViewModelsTable>>;

/** D6. The list rows and the detail summary carry `Optimization`, whose `status` is the UI pill status: a superset
 *  of what the daemon sends (it also has `merged` / `blocked`, which only the provisional overview produces). */
type StatusRowsTable = {
  UiStatusIsRenderable: Within<S['UiStatus'], OptimizationStatus>;
  OptimizationStatusIsRenderable: Within<S['OptimizationStatus'], OptimizationStatus>;
  SnapshotRowStatus: Within<S['OptimizationsSnapshot']['optimizations'][number]['status'], Optimization['status']>;
  OptimizationRowStatus: Within<S['OptimizationVm']['status'], Optimization['status']>;
  DetailSummary: Within<S['OptimizationDetail']['summary']['status'], OptimizationDetail['summary']['status']>;
};
export type StatusRows = Expect<AssertAll<StatusRowsTable>>;

// ---------------------------------------------------------------------------------------------------------------
// 2. Events: the SSE union against `EngineEventVM`.

type WireEvent = S['ApiEvent'];
type TsEventType = EngineEventVM['type'];
type Wire<K extends WireEvent['type']> = ByType<WireEvent, K>;
type Ts<K extends TsEventType> = ByType<EngineEventVM, K>;

/** D7. Wire-only events: the stream's own control events and the provisional groups. */
type WireOnlyEvents = 'hello' | 'resync' | 'github' | 'environment-draft' | 'connector';

type EventsTable = {
  // Every TypeScript event exists on the wire, and the wire has nothing else but `WireOnlyEvents`.
  TsEventsAreOnTheWire: [TsEventType] extends [WireEvent['type']] ? true : { missing: Exclude<TsEventType, WireEvent['type']> };
  WireOnlyEventsAreListed: [Exclude<WireEvent['type'], TsEventType>] extends [WireOnlyEvents]
    ? [WireOnlyEvents] extends [Exclude<WireEvent['type'], TsEventType>]
      ? true
      : { listedButNotOnTheWire: Exclude<WireOnlyEvents, Exclude<WireEvent['type'], TsEventType>> }
    : { unlisted: Exclude<Exclude<WireEvent['type'], TsEventType>, WireOnlyEvents> };
  // D8. The wire's `optimization.status` is the closed ledger enum, the TypeScript event says `string`.
  optimization: Within<Wire<'optimization'>, Ts<'optimization'>>;
  // D9. The wire's `agent-event` carries the log line (`event`) so a log view can append without refetching.
  'agent-event': Same<Ts<'agent-event'>, Omit<Wire<'agent-event'>, 'event'>>;
  candidate: Same<Ts<'candidate'>, Wire<'candidate'>>;
  ratchet: Same<Ts<'ratchet'>, Wire<'ratchet'>>;
  spend: Same<Ts<'spend'>, Wire<'spend'>>;
  notice: Same<Ts<'notice'>, Wire<'notice'>>;
  'run-active': Same<Ts<'run-active'>, Wire<'run-active'>>;
  sandbox: Same<Ts<'sandbox'>, Wire<'sandbox'>>;
};
export type Events = Expect<AssertAll<EventsTable>>;

// ---------------------------------------------------------------------------------------------------------------
// 3. Request bodies and query parameters (what the TypeScript client sends must be accepted).

/** D10. Fields of features the desktop app does not have (or has not got yet) that the daemon already accepts. */
type CreateOptimizationExtra = 'connectors' | 'environmentDraftId';
/** D11. Template parameters the desktop app does not offer: the starting direction, metric and value. */
type ProposeTemplateExtra = 'direction' | 'metric' | 'valueUsd';

type Arg<M extends keyof RatchetApi, I extends number> = Parameters<RatchetApi[M]>[I & keyof Parameters<RatchetApi[M]>];

type RequestsTable = {
  CreateOptimizationInput: Sends<CreateOptimizationInput, S['CreateOptimizationInput'], CreateOptimizationExtra>;
  PreflightInput: Sends<PreflightInput, S['PreflightInput']>;
  ProposeTemplateInput: Sends<ProposeTemplateInput, S['ProposeTemplateInput'], ProposeTemplateExtra>;
  LiveSettingsInput: Sends<LiveSettingsInput, S['LiveSettingsInput']>;
  RepoSourceInput: Same<RepoSourceInput, S['RepoSource']>;
  // the `RatchetApi` method arguments that are not a whole input type
  messageAgent: Same<{ text: Arg<'messageAgent', 1> }, Body<'messageAgent'>>;
  stopOptimization: Sends<{ reason?: Arg<'stopOptimization', 1> }, Body<'stopOptimization'>>;
  setAwsProfile: Same<{ profile: Arg<'setAwsProfile', 0> }, Body<'setAwsProfile'>>;
  setSecret: Same<{ value: Arg<'setSecret', 1> }, Body<'setSecret'>>;
  setSecretName: Same<Arg<'setSecret', 0>, Ops['setSecret']['parameters']['path']['name']>;
  clearSecretName: Same<Arg<'clearSecret', 0>, Ops['clearSecret']['parameters']['path']['name']>;
  testConnection: Same<{ target: Arg<'testConnection', 0> }, Body<'testConnection'>>;
  validateConfig: Same<{ yaml: Arg<'validateConfig', 0> }, Body<'validateConfig'>>;
  // query parameters
  listOptimizationsRange: Same<Opt<Opt<Arg<'listOptimizations', 0>>['range']>, Opt<Query<'listOptimizations'>['range']>>;
  ratchetHistoryMetric: Same<Opt<Arg<'ratchetHistory', 1>>, Opt<Query<'getRatchetHistory'>['metric']>>;
  recentActivityOptimizationId: Same<Opt<Opt<Arg<'recentActivity', 0>>['optimizationId']>, Opt<Query<'listActivity'>['optimizationId']>>;
  recentActivityLimit: Same<Opt<Opt<Arg<'recentActivity', 0>>['limit']>, Opt<Query<'listActivity'>['limit']>>;
  agentEventsCursor: Same<Opt<Arg<'agentEvents', 1>>, Opt<Query<'listAgentEvents'>['cursor']>>;
  agentEventsLimit: Same<Opt<Arg<'agentEvents', 2>>, Opt<Query<'listAgentEvents'>['limit']>>;
  listReposQuery: Same<Opt<Arg<'listRepos', 0>>, Opt<Query<'listRepos'>['query']>>;
};
export type Requests = Expect<AssertAll<RequestsTable>>;

// ---------------------------------------------------------------------------------------------------------------
// 4. Operations: what each `RatchetApi` method resolves to is what the operation that replaces it returns
//    (the table of method -> operation is also in crates/ratchetd/tests/api/electron_api.rs).

type Returns<M extends keyof RatchetApi> = RatchetApi[M] extends (...a: never[]) => Promise<infer R> ? R : never;

type OperationsTable = {
  listOptimizations: Same<Omit<Returns<'listOptimizations'>, 'optimizations' | 'engine'>, Omit<Reply<'listOptimizations'>, 'optimizations' | 'engine'>>;
  getOptimization: Same<Omit<Returns<'getOptimization'>, 'summary'>, Omit<Reply<'getOptimization'>, 'summary'>>;
  ratchetHistory: Same<Returns<'ratchetHistory'>, Reply<'getRatchetHistory'>>;
  winningChange: Same<Returns<'winningChange'>, Reply<'getWinningChange'>>;
  candidatesWithResults: Same<Returns<'candidatesWithResults'>, Reply<'listCandidates'>>;
  spendBreakdown: Same<Returns<'spendBreakdown'>, Reply<'getSpend'>>;
  recentActivity: Same<Returns<'recentActivity'>, Reply<'listActivity'>>;
  agentEvents: Same<Returns<'agentEvents'>, Reply<'listAgentEvents'>>;
  validateConfig: Same<ValidateWire, Reply<'validateConfig'>>;
  preflight: Same<Returns<'preflight'>, Reply<'runPreflight'>>;
  getSettings: Same<SettingsWire, Reply<'getSettings'>>;
  environments: Same<Returns<'environments'>, Reply<'listEnvironments'>>;
  githubStatus: Same<Returns<'githubStatus'>, Reply<'getGithubStatus'>>;
  githubSignIn: Same<Returns<'githubSignIn'>, Reply<'startGithubSignIn'>>;
  githubSignOut: Same<Returns<'githubSignOut'>, Reply<'signOutOfGithub'>>;
  listRepos: Same<Returns<'listRepos'>, Reply<'listRepos'>>;
  inspectRepo: Same<Omit<Returns<'inspectRepo'>, 'templates'>, Omit<Reply<'inspectRepo'>, 'templates'>>;
  awsProfiles: Same<Returns<'awsProfiles'>, Reply<'listAwsProfiles'>>;
  detectHarnesses: Same<Returns<'detectHarnesses'>, Reply<'listHarnesses'>>;
  redetectHarnesses: Same<Returns<'redetectHarnesses'>, Reply<'detectHarnesses'>>;
  listTemplates: Same<Returns<'listTemplates'>, Omit<Reply<'listTemplates'>[number], TemplateExtra>[]>;
  proposeTemplate: Same<Returns<'proposeTemplate'>, Reply<'proposeTemplate'>>;
  // D12. `createOptimization` resolves to the id, the daemon answers `{id}`.
  createOptimization: Same<{ id: Returns<'createOptimization'> }, Reply<'createOptimization'>>;
  openPullRequest: Same<Returns<'openPullRequest'>, Reply<'openPullRequest'>>;
  endSandbox: Same<Returns<'endSandbox'>, Reply<'endEnvironment'>>;
  setAwsProfile: Same<SettingsWire, Reply<'setAwsProfile'>>;
  setSecret: Same<SettingsWire, Reply<'setSecret'>>;
  clearSecret: Same<SettingsWire, Reply<'clearSecret'>>;
  saveLiveSettings: Same<SettingsWire, Reply<'saveAdvancedSettings'>>;
  testConnection: Same<Returns<'testConnection'>, Reply<'testConnection'>>;
};
export type Operations = Expect<AssertAll<OperationsTable>>;

// ---------------------------------------------------------------------------------------------------------------
// 5. Not checked yet. Each group is being rewritten on its own branch; when it lands, move its line into the tables
//    above (one line per type). Schemas: S['OverviewVm'], S['ConnectorVm'], S['ConnectorSelectionVm'],
//    S['EnvironmentDraftVm'] and their inputs.

export const PENDING = {
  // TODO(claude/ts-overview-shell): OverviewVM <-> S['OverviewVm'] (+ NeedsYouItem, OverviewRow, hero cards, staircase)
  overview: 'pending',
  // TODO(claude/ts-connectors): ConnectorVM <-> S['ConnectorVm'], CreateConnectorInput, UpdateConnectorInput, ConnectorTestVm
  connectors: 'pending',
  // TODO(claude/ts-connectors): ConnectorSelectionVM <-> S['ConnectorSelectionVm'], SaveConnectorSelectionInput
  connectorSelections: 'pending',
  // TODO(claude/ts-dynamic-env): EnvironmentDraftVM <-> S['EnvironmentDraftVm'], CreateEnvironmentDraftInput,
  // ReviseEnvironmentDraftInput, EnvFilesVm, plus the `environment-draft` and `connector` events
  environmentDrafts: 'pending',
} as const;

// Not part of `RatchetApi` (so nothing to compare): daemon status, calibration, protected paths, cost reconciliation,
// `getTemplate`, `getReport` (D13: `exportReport` is the app's save dialog around the markdown).
