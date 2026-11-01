//! Property tests of the ledger's invariants: the ratchet only turns one way, spend adds up, and the append-only
//! tables stay append-only.

mod common;

use common::*;
use proptest::prelude::*;
use ratchet_core::enums::{AgentKind, Direction, SpendCategory};
use ratchet_core::rows::is_improvement;
use ratchet_ledger::{AddAgentInput, RecordSpendInput, SubmitCandidateInput};
use sqlx::Row;

fn run<F: std::future::Future<Output = ()>>(f: F) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f);
}

fn category() -> impl Strategy<Value = SpendCategory> {
    prop::sample::select(SpendCategory::ALL.to_vec())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 40, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn the_ratchet_only_turns_one_way(minimize in any::<bool>(), start in 1.0f64..1000.0, attempts in prop::collection::vec(1.0f64..1000.0, 1..25)) {
        run(async {
            let direction = if minimize { Direction::Minimize } else { Direction::Maximize };
            let l = ledger().await;
            let mut input = opt_input();
            input.direction = direction;
            let s = setup_with(&l, input).await;
            // the baseline of `setup` is 100: re-record it at `start`
            mk_baseline(&l, &s.opt.id, start).await;

            let mut best = start;
            let mut accepted = Vec::new();
            for (i, v) in attempts.iter().enumerate() {
                let c = mk_candidate(&l, &s.opt.id, &s.claude.id, &format!("sha{i}")).await;
                let outcome = l.move_ratchet(&s.opt.id, &c.id, *v, None).await;
                if is_improvement(direction, best, *v) {
                    let m = outcome.expect("a strict improvement moves the ratchet");
                    assert_eq!((m.from_value, m.to_value), (best, *v));
                    best = *v;
                    accepted.push(*v);
                } else {
                    assert!(outcome.is_err(), "{v} must not move the ratchet from {best}");
                }
                assert_eq!(l.current_best(&s.opt.id).await.unwrap(), Some(best));
            }
            // the history is a chain of strictly improving steps from the baseline
            let hist = l.ratchet_history(&s.opt.id).await.unwrap();
            assert_eq!(hist.len(), accepted.len());
            let mut prev = start;
            for (h, v) in hist.iter().zip(&accepted) {
                assert_eq!((h.from_value, h.to_value), (prev, *v));
                assert!(is_improvement(direction, h.from_value, h.to_value));
                prev = h.to_value;
            }
        });
    }

    #[test]
    fn spend_adds_up_by_category_agent_and_kind(events in prop::collection::vec((category(), -5.0f64..50.0, any::<bool>(), 0usize..3), 0..30)) {
        run(async {
            let l = ledger().await;
            let s = setup(&l).await;
            let agents = [Some(s.claude.id.clone()), Some(s.codex.id.clone()), None];
            let mut want_total = 0.0;
            let mut want_estimated = 0.0;
            for (category, usd, estimated, who) in &events {
                let mut i = RecordSpendInput::new(s.opt.id.clone(), *category, *usd, "p");
                i.agent_id = agents[*who].clone();
                i.estimated = Some(*estimated);
                l.record_spend(i).await.unwrap();
                want_total += usd;
                if *estimated {
                    want_estimated += usd;
                }
            }
            let tol = 1e-9 * (1.0 + events.len() as f64 * 50.0);
            let b = l.spend_breakdown(&s.opt.id).await.unwrap();
            assert!((b.total - want_total).abs() < tol);
            assert!((b.estimated - want_estimated).abs() < tol);
            assert!((b.estimated + b.actual - b.total).abs() < tol);
            assert!((b.by_category.total() - b.total).abs() < tol);
            assert!((b.by_agent.iter().map(|a| a.total).sum::<f64>() - b.total).abs() < tol);
            assert!((l.total_spend(&s.opt.id).await.unwrap() - want_total).abs() < tol);
            for a in &b.by_agent {
                assert!((a.by_category.total() - a.total).abs() < tol);
            }
        });
    }

    #[test]
    fn the_append_only_tables_reject_every_update_and_delete(moves in 1usize..5, spends in 1usize..6) {
        run(async {
            let l = ledger().await;
            let s = setup(&l).await;
            for i in 0..moves {
                let c = mk_candidate(&l, &s.opt.id, &s.claude.id, &format!("m{i}")).await;
                l.move_ratchet(&s.opt.id, &c.id, 99.0 - i as f64, None).await.unwrap();
            }
            for i in 0..spends {
                l.record_spend(RecordSpendInput::new(s.opt.id.clone(), SpendCategory::Tokens, i as f64, "p")).await.unwrap();
            }
            for sql in [
                "update ratchet_moves set to_value = 0",
                "delete from ratchet_moves",
                "update spend_events set usd = 0",
                "delete from spend_events",
            ] {
                assert!(sqlx::query(sql).execute(l.pool()).await.is_err(), "{sql}");
            }
            let n: i64 = sqlx::query("select (select count(*) from ratchet_moves) + (select count(*) from spend_events) as n")
                .fetch_one(l.pool()).await.unwrap().get("n");
            assert_eq!(n as usize, moves + spends);
        });
    }

    #[test]
    fn agents_come_back_in_the_order_they_were_added(ids in prop::collection::vec("[a-z]{1,6}", 1..8)) {
        run(async {
            let l = ledger().await;
            let o = mk_opt(&l).await;
            let mut unique = Vec::new();
            for id in ids {
                if unique.contains(&id) {
                    continue;
                }
                let mut a = AddAgentInput::new(o.id.clone(), AgentKind::Mock, format!("b/{id}"));
                a.id = Some(id.clone().into());
                a.created_at = Some(7); // all at once
                l.add_agent(a).await.unwrap();
                unique.push(id);
            }
            let got: Vec<String> = l.list_agents(&o.id).await.unwrap().into_iter().map(|a| a.id.to_string()).collect();
            assert_eq!(got, unique);
            let _ = SubmitCandidateInput::new(o.id.clone(), "x", "y");
        });
    }
}
