CREATE TABLE `agent_events` (
	`id` integer PRIMARY KEY AUTOINCREMENT NOT NULL,
	`agent_id` text NOT NULL,
	`at` integer NOT NULL,
	`type` text NOT NULL,
	`payload` text,
	FOREIGN KEY (`agent_id`) REFERENCES `agents`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE INDEX `agent_events_agent_idx` ON `agent_events` (`agent_id`,`id`);--> statement-breakpoint
CREATE TABLE `agents` (
	`id` text PRIMARY KEY NOT NULL,
	`optimization_id` text NOT NULL,
	`kind` text NOT NULL,
	`branch` text NOT NULL,
	`runner_instance_id` text,
	`status` text DEFAULT 'pending' NOT NULL,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`optimization_id`) REFERENCES `optimizations`(`id`) ON UPDATE no action ON DELETE no action,
	CONSTRAINT "agents_kind_check" CHECK("agents"."kind" in ('claude-code', 'codex', 'mock')),
	CONSTRAINT "agents_status_check" CHECK("agents"."status" in ('pending', 'launching', 'running', 'paused', 'stopped', 'done', 'failed'))
);
--> statement-breakpoint
CREATE INDEX `agents_optimization_idx` ON `agents` (`optimization_id`);--> statement-breakpoint
CREATE TABLE `baselines` (
	`optimization_id` text PRIMARY KEY NOT NULL,
	`samples` text NOT NULL,
	`value` real NOT NULL,
	`noise_floor` real NOT NULL,
	`stats` text NOT NULL,
	`recorded_at` integer NOT NULL,
	FOREIGN KEY (`optimization_id`) REFERENCES `optimizations`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE TABLE `bench_results` (
	`id` integer PRIMARY KEY AUTOINCREMENT NOT NULL,
	`candidate_id` text NOT NULL,
	`phase` text NOT NULL,
	`samples` text NOT NULL,
	`stats` text NOT NULL,
	`guardrails` text DEFAULT '[]' NOT NULL,
	`started_at` integer NOT NULL,
	`finished_at` integer NOT NULL,
	FOREIGN KEY (`candidate_id`) REFERENCES `candidates`(`id`) ON UPDATE no action ON DELETE no action,
	CONSTRAINT "bench_results_phase_check" CHECK("bench_results"."phase" in ('bench', 'confirm'))
);
--> statement-breakpoint
CREATE INDEX `bench_results_candidate_idx` ON `bench_results` (`candidate_id`);--> statement-breakpoint
CREATE TABLE `candidates` (
	`id` text PRIMARY KEY NOT NULL,
	`optimization_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`sha` text NOT NULL,
	`message` text DEFAULT '' NOT NULL,
	`submitted_at` integer NOT NULL,
	`verdict` text DEFAULT 'pending' NOT NULL,
	`verdict_at` integer,
	`reasons` text DEFAULT '[]' NOT NULL,
	FOREIGN KEY (`optimization_id`) REFERENCES `optimizations`(`id`) ON UPDATE no action ON DELETE no action,
	FOREIGN KEY (`agent_id`) REFERENCES `agents`(`id`) ON UPDATE no action ON DELETE no action,
	CONSTRAINT "candidates_verdict_check" CHECK("candidates"."verdict" in ('pending', 'verified', 'rejected', 'failed_confirm', 'blocked'))
);
--> statement-breakpoint
CREATE INDEX `candidates_optimization_idx` ON `candidates` (`optimization_id`,`submitted_at`);--> statement-breakpoint
CREATE INDEX `candidates_agent_idx` ON `candidates` (`agent_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `candidates_agent_sha_unique` ON `candidates` (`agent_id`,`sha`);--> statement-breakpoint
CREATE TABLE `engine_kv` (
	`key` text PRIMARY KEY NOT NULL,
	`value` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `optimizations` (
	`id` text PRIMARY KEY NOT NULL,
	`name` text,
	`repo` text NOT NULL,
	`base_ref` text NOT NULL,
	`metric` text NOT NULL,
	`direction` text NOT NULL,
	`min_gain` real NOT NULL,
	`budget_usd` real NOT NULL,
	`time_limit_s` integer NOT NULL,
	`status` text DEFAULT 'draft' NOT NULL,
	`created_at` integer NOT NULL,
	`started_at` integer,
	`ended_at` integer,
	`config_yaml` text NOT NULL,
	`pack` text,
	`objective_name` text,
	`evaluator_kind` text DEFAULT 'benchmark' NOT NULL,
	`value_usd_per_unit` real,
	`value_period` text,
	`value_basis` text,
	`value_unit` text,
	`sandbox_id` text,
	FOREIGN KEY (`sandbox_id`) REFERENCES `sandboxes`(`id`) ON UPDATE no action ON DELETE no action,
	CONSTRAINT "optimizations_direction_check" CHECK("optimizations"."direction" in ('minimize', 'maximize')),
	CONSTRAINT "optimizations_status_check" CHECK("optimizations"."status" in ('draft', 'baselining', 'running', 'stopping', 'completed', 'stopped', 'failed')),
	CONSTRAINT "optimizations_evaluator_kind_check" CHECK("optimizations"."evaluator_kind" in ('benchmark', 'backtest', 'eval', 'simulation')),
	CONSTRAINT "optimizations_value_period_check" CHECK("optimizations"."value_period" is null or "optimizations"."value_period" in ('month', 'year')),
	CONSTRAINT "optimizations_value_basis_check" CHECK("optimizations"."value_basis" is null or "optimizations"."value_basis" in ('estimated', 'measured')),
	CONSTRAINT "optimizations_value_usd_check" CHECK("optimizations"."value_usd_per_unit" is null or "optimizations"."value_usd_per_unit" > 0)
);
--> statement-breakpoint
CREATE INDEX `optimizations_sandbox_idx` ON `optimizations` (`sandbox_id`);--> statement-breakpoint
CREATE TABLE `ratchet_moves` (
	`id` integer PRIMARY KEY AUTOINCREMENT NOT NULL,
	`optimization_id` text NOT NULL,
	`candidate_id` text NOT NULL,
	`from_value` real NOT NULL,
	`to_value` real NOT NULL,
	`at` integer NOT NULL,
	FOREIGN KEY (`optimization_id`) REFERENCES `optimizations`(`id`) ON UPDATE no action ON DELETE no action,
	FOREIGN KEY (`candidate_id`) REFERENCES `candidates`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE INDEX `ratchet_moves_optimization_idx` ON `ratchet_moves` (`optimization_id`,`id`);--> statement-breakpoint
CREATE UNIQUE INDEX `ratchet_moves_candidate_unique` ON `ratchet_moves` (`candidate_id`);--> statement-breakpoint
CREATE TABLE `sandboxes` (
	`id` text PRIMARY KEY NOT NULL,
	`provider` text NOT NULL,
	`region` text NOT NULL,
	`account_id` text,
	`status` text DEFAULT 'planned' NOT NULL,
	`outputs` text,
	`created_at` integer NOT NULL,
	`applied_at` integer,
	`destroyed_at` integer,
	CONSTRAINT "sandboxes_provider_check" CHECK("sandboxes"."provider" in ('aws', 'mock')),
	CONSTRAINT "sandboxes_status_check" CHECK("sandboxes"."status" in ('planned', 'applying', 'applied', 'destroying', 'destroyed', 'failed'))
);
--> statement-breakpoint
CREATE TABLE `spend_events` (
	`id` integer PRIMARY KEY AUTOINCREMENT NOT NULL,
	`optimization_id` text NOT NULL,
	`agent_id` text,
	`category` text NOT NULL,
	`usd` real NOT NULL,
	`estimated` integer DEFAULT true NOT NULL,
	`source` text NOT NULL,
	`at` integer NOT NULL,
	FOREIGN KEY (`optimization_id`) REFERENCES `optimizations`(`id`) ON UPDATE no action ON DELETE no action,
	FOREIGN KEY (`agent_id`) REFERENCES `agents`(`id`) ON UPDATE no action ON DELETE no action,
	CONSTRAINT "spend_events_category_check" CHECK("spend_events"."category" in ('agent_runner', 'experiment', 'bench_runner', 'storage_egress', 'tokens'))
);
--> statement-breakpoint
CREATE INDEX `spend_events_optimization_idx` ON `spend_events` (`optimization_id`);
--> statement-breakpoint
-- Append-only ledgers: ratchet_moves and spend_events reject UPDATE and DELETE.
CREATE TRIGGER `ratchet_moves_no_update` BEFORE UPDATE ON `ratchet_moves`
BEGIN
	SELECT RAISE(ABORT, 'ratchet_moves is append-only');
END;
--> statement-breakpoint
CREATE TRIGGER `ratchet_moves_no_delete` BEFORE DELETE ON `ratchet_moves`
BEGIN
	SELECT RAISE(ABORT, 'ratchet_moves is append-only');
END;
--> statement-breakpoint
CREATE TRIGGER `spend_events_no_update` BEFORE UPDATE ON `spend_events`
BEGIN
	SELECT RAISE(ABORT, 'spend_events is append-only');
END;
--> statement-breakpoint
CREATE TRIGGER `spend_events_no_delete` BEFORE DELETE ON `spend_events`
BEGIN
	SELECT RAISE(ABORT, 'spend_events is append-only');
END;
