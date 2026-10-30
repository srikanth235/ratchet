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
