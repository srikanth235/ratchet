# 1. Does the Home composer replace the New optimization wizard?

Status: proposed (F2, for the owner to accept before P5 starts). Context: srikanth235/ratchet#10 §1, `docs/AGENT_HARNESS_PLAN.md` §4.8.

## Context

Today a person creates an optimization in a four-step wizard (Goal and repo, Connections, Environment, Budget and time:
`NewOptimizationFlow`, `Screens/NewOptimization*`). The v0.3 design adds a Home composer and a creation conversation (s8): the
person says what should get better, the chat drafts an optimization as a card, and "Create" calls `create_optimization`. Both
produce the same thing, an optimization that starts running, so the app should not keep two creation paths that drift apart.

## Decision

The composer and the draft card become the way to create an optimization. The wizard's *screens* go away when P5 ships them; its
*logic* stays:

* `NewOptimizationFlow`, `RepoInspection`, `Preflight*`, `TemplateProposalFlow` and the environment draft calls are what the
  daemon-side `draft_optimization` tool and the card's rows are built from. They keep their tests and remain the engine for
  "Use template" and the repo checks.
* When no harness is signed in (`POST /v1/chats` answers 409) the composer cannot draft. Then Home shows the Agents page's
  sign-in buttons first; it does not fall back to the wizard, because the wizard also needs an agent for the environment draft.
* The `Page` enum already carries `home` and `agents` (hidden until their screens exist, `Page.isAvailable`). Turning them on and
  removing `router.wizard` is one commit in P5.

## Consequences

* One creation path to test and to explain; the card's rows are the only place settings are edited at creation.
* The wizard keeps working until the composer is ready, so nothing regresses in between.
* If the conversation turns out to be too slow for people who know exactly what they want, the answer is a faster first draft
  (templates, remembered defaults), not a second flow.

## Considered, not chosen

* Keep both flows for good: two places to fix every creation bug, and the draft card would have to be duplicated as a wizard step.
* Replace the wizard now: the composer needs P5's screens and the chat engine's UI; removing the wizard first would leave no way to
  start an optimization.
