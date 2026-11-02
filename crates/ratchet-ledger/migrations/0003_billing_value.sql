-- Cost split and value story (P7). Not part of the drizzle schema 0001 was ported from.
--
-- spend_events: how the tokens were paid for and how many there were. `billing = 'subscription'` rows are the user's own plan on
-- this Mac: tokens are counted, `usd` is always 0, so every sum of `usd` is real dollars. Rows written before this migration are
-- 'api' (a key, billed per token), with no token counts. The append-only triggers only reject UPDATE and DELETE, so adding
-- columns is fine.
ALTER TABLE `spend_events` ADD COLUMN `billing` text DEFAULT 'api' NOT NULL CHECK(`billing` in ('api', 'subscription'));
ALTER TABLE `spend_events` ADD COLUMN `input_tokens` integer DEFAULT 0 NOT NULL;
ALTER TABLE `spend_events` ADD COLUMN `output_tokens` integer DEFAULT 0 NOT NULL;

-- optimizations: the plain-words half of the value model ("ms saved x checkout volume") and how often it runs.
ALTER TABLE `optimizations` ADD COLUMN `value_note` text;
ALTER TABLE `optimizations` ADD COLUMN `schedule` text CHECK(`schedule` is null or `schedule` in ('once', 'nightly', 'weekdays', 'weekly', 'on-push'));
