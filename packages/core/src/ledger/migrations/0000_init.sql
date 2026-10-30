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
	CONSTRAINT "agents_kind_check" CHECK("agents"."kind" in ('claude-code', 'codex')),
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
	CONSTRAINT "optimizations_direction_check" CHECK("optimizations"."direction" in ('minimize', 'maximize')),
	CONSTRAINT "optimizations_status_check" CHECK("optimizations"."status" in ('draft', 'baselining', 'running', 'stopping', 'completed', 'stopped', 'failed'))
);
--> statement-breakpoint
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