PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_agents` (
	`id` text PRIMARY KEY NOT NULL,
	`optimization_id` text NOT NULL,
	`kind` text NOT NULL,
	`branch` text NOT NULL,
	`runner_instance_id` text,
	`status` text DEFAULT 'pending' NOT NULL,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`optimization_id`) REFERENCES `optimizations`(`id`) ON UPDATE no action ON DELETE no action,
	CONSTRAINT "agents_kind_check" CHECK("__new_agents"."kind" in ('claude-code', 'codex', 'mock')),
	CONSTRAINT "agents_status_check" CHECK("__new_agents"."status" in ('pending', 'launching', 'running', 'paused', 'stopped', 'done', 'failed'))
);
--> statement-breakpoint
INSERT INTO `__new_agents`("id", "optimization_id", "kind", "branch", "runner_instance_id", "status", "created_at") SELECT "id", "optimization_id", "kind", "branch", "runner_instance_id", "status", "created_at" FROM `agents`;--> statement-breakpoint
DROP TABLE `agents`;--> statement-breakpoint
ALTER TABLE `__new_agents` RENAME TO `agents`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
CREATE INDEX `agents_optimization_idx` ON `agents` (`optimization_id`);