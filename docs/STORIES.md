# Stories — v0.3.1 flows

Acceptance criteria for the flows that F1 added or changed in `design/v03.html` (21 screens, plus `s22`). Each line starts with how it is checked:
`[snap]` snapshot against the mockup, `[unit]` unit test, `[self]` app self-test, `[manual]` a person tries it. P5 and F4 build these; this file is
the contract. Terms are in `docs/GLOSSARY.md`.

### S-101 Create an optimization that runs at once
As the user, I press Create Optimization on the draft card and the first run starts without waiting on a pull request.
Screens: 8-new-optimization.png, 18-new-optimization-error.png
Acceptance:
- [snap] The Config row reads `ratchet.yaml · bench/latency.js · … → branch ratchet/config`; the footer reads "Runs at once from ratchet/config. A PR adopts it into main later."
- [unit] Create writes the config to the `ratchet/config` branch and starts a run; no state named "waiting" or "approve" exists.
- [self] Create and Run Once starts one run and does not add a schedule; its button shows the estimated cost ("≈$38").
- [snap] While the environment check fails, both create buttons are disabled and an inline error offers Use Smaller Setup and Try Again.
UX:
- Flow: send prompt → steps stream → card → Create Optimization (or Create and Run Once · ≈$38) → Optimization page, run #1 live
- Surface: inline card in the conversation (changes values already on screen)
- States: drafting (Stop in the composer, 22), ready (8), error with fix (18)
- Words: Create Optimization · Create and Run Once · Start Over · Use Smaller Setup · Try Again
- Access: every row button has a distinct label ("Change agents"); Return in the composer sends, ⌘↩ too

### S-102 Change one row of the draft in a popover
As the user, I press "Change agents" and a popover lets me reorder agents and pick where they run.
Acceptance:
- [snap] The popover is titled "Change agents", lists agents in order with Move Up and Move Down (28 px), a Where switch (This Mac / Cloud) and Done.
- [self] After a change the row shows "Updated" and Undo; Undo restores the old value; Escape closes the popover and returns focus to the button.
- [snap] The agent order reads "Claude Code → Codex → Grok · on this Mac" on the card, the optimization page and the Agents page.

### S-103 Start Over and Stop while drafting
Acceptance:
- [self] While the setup agent drafts, the composer's send button becomes Stop; Stop ends the draft and keeps the steps so far.
- [self] Start Over clears the conversation and returns to Home with the prompt restored.

### S-104 Steer a running run, and see it was read
As the user, I leave a note on a run, see when the agent read it, and can withdraw it until then.
Screens: 2-run.png
Acceptance:
- [snap] The composer is multi-line ("Return adds a line · ⌘↩ sends").
- [snap] The timeline shows "You · “…” Queued · read at the next candidate" with Undo, then "Read by Claude Code at 02:11".
- [unit] Undo works only while the note is unread.

### S-105 Run header: status, Stop, Pause Schedule, Open PR
Acceptance:
- [snap] The header shows a status word with a shape, Stop, Pause Schedule and Open PR #n.
- [self] Stop asks "Stop Checkout p95?" with "Wins found so far are kept." and buttons Cancel and Stop (19).
- [self] Pause Schedule becomes Resume Schedule; on a single agent the button is Pause Agent (20).
- [snap] The log switch reads Overview | By Agent | Everything, each with a tooltip; an agent that has not started shows an empty state.

### S-106 Edit an optimization's settings in place
Screens: 3-optimization.png
Acceptance:
- [snap] Each row has a visible Edit button, a "Using default" or "Custom" tag, and the rows Measures, Agents, Cloud, Each run, Schedule, Value, Output, Notify.
- [self] Edit opens a popover; saving shows "Saved" beside the row.
- [self] Archive… confirms, stops the schedule and keeps history.
- [snap] The Overview footnote's "Edit value" link opens this page.

### S-107 One value story
Acceptance:
- [snap] Home and Overview both say "$12.5k delivered + $2.9k pending · 68×"; "locked in" and "$15.4k" appear nowhere.
- [unit] Pending value counts only open PRs and never enters Return.

### S-108 See that the service is down
Screens: 12-daemon-down.png
Acceptance:
- [snap] A banner on every page says "Ratchet service isn't running." with Start Service and the last-updated time; page content is dimmed.
- [self] New Optimization and other daemon actions are disabled while it shows; the banner clears when the service answers.

### S-109 Fix a failed connector next to its status
Screens: 13-failed-connector.png, 7-agents.png
Acceptance:
- [snap] Every non-OK status shows a shape, a word, a one-line reason and one button (Sign In, Reconnect, Fix, Add Key…, Re-detect).
- [self] Sign In starts the sign-in flow; it does not only show a hint.

### S-110 First launch, loading and empty states
Screens: 11-empty-first-launch.png, 14-loading.png
Acceptance:
- [snap] Overview with no data says "No wins yet" and "Start your first optimization." with one New Optimization button.
- [snap] Loading shows skeletons, "Loading your optimizations…" and disabled New.

### S-111 Settings window
Screens: 6-settings.png, 21-settings-this-mac.png
Acceptance:
- [snap] ⌘, opens a 520 pt window with four tabs: General, Defaults, Notifications, This Mac; no Templates, Connectors or Agents.
- [snap] Defaults are labelled "Default for new optimizations".
- [self] Switching Demo mode asks first ("Switch to Demo mode?"); Stop… on the service asks first.
- [manual] The app window dims (grey traffic lights) while Settings is key.

### S-112 Light mode
Screens: 15-home-light.png, 16-overview-light.png, 17-run-light.png
Acceptance:
- [snap] Home, Overview and Run render in light with text at 4.5:1, control borders at 3:1 and brass text in `brassText`.
