-- Chat (P4): one row per conversation, and its messages in order. Not part of the drizzle schema 0001 was ported from.
-- Rows are plain data: no secret ever goes in (the MCP session token lives in memory and is never stored).

CREATE TABLE `chats` (
	`id` text PRIMARY KEY NOT NULL,
	`repo` text,
	`optimization_id` text,
	`harness` text NOT NULL,
	`model` text,
	`cli_session_id` text,
	`status` text DEFAULT 'open' NOT NULL,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	CONSTRAINT "chats_status_check" CHECK("chats"."status" in ('open', 'closed'))
);

CREATE INDEX `chats_updated_idx` ON `chats` (`updated_at`);

CREATE TABLE `chat_messages` (
	`chat_id` text NOT NULL,
	`seq` integer NOT NULL,
	`role` text NOT NULL,
	`text` text DEFAULT '' NOT NULL,
	`harness` text,
	`model` text,
	`card` text,
	`usage` text,
	`at` integer NOT NULL,
	PRIMARY KEY(`chat_id`, `seq`),
	FOREIGN KEY (`chat_id`) REFERENCES `chats`(`id`) ON UPDATE no action ON DELETE cascade,
	CONSTRAINT "chat_messages_role_check" CHECK("chat_messages"."role" in ('user', 'assistant', 'tool', 'card', 'note'))
);
