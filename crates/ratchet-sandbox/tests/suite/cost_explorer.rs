//! Port of `packages/aws/src/cost-explorer.test.ts`.

use crate::common::{Log, meta, rec};
use aws_sdk_costexplorer::operation::get_cost_and_usage::{
    GetCostAndUsageError, GetCostAndUsageInput, GetCostAndUsageOutput,
};
use aws_sdk_costexplorer::types::{
    DateInterval, Granularity, Group, GroupDefinitionType, MatchOption, MetricValue, ResultByTime,
};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use ratchet_sandbox::cost_explorer::{
    AwsCostReconciler, MockCostReconciler, billed, category_for_tags,
};
use ratchet_sandbox::domain::{CostActualsQuery, SpendCategory};
use ratchet_sandbox::ports::CostSource;

const DAY: i64 = 86_400_000;
const T: i64 = 1_793_194_200_000; // 2026-10-28 13:30 UTC

fn query() -> CostActualsQuery {
    CostActualsQuery {
        optimization_id: "opt-1".into(),
        start_ms: T,
        end_ms: T + DAY / 2,
    } // ends 2026-10-29 01:30 UTC
}

fn group(role: &str, agent: &str, amount: &str) -> Group {
    Group::builder()
        .keys(format!("ratchet:role${role}"))
        .keys(format!("ratchet:agent${agent}"))
        .metrics(
            "UnblendedCost",
            MetricValue::builder().amount(amount).unit("USD").build(),
        )
        .build()
}

fn day(start: &str, groups: Vec<Group>, estimated: bool) -> ResultByTime {
    let mut b = ResultByTime::builder()
        .time_period(
            DateInterval::builder()
                .start(start)
                .end(start)
                .build()
                .unwrap(),
        )
        .estimated(estimated);
    for g in groups {
        b = b.groups(g);
    }
    b.build()
}

fn client_returning(
    outputs: Vec<GetCostAndUsageOutput>,
    log: &Log<GetCostAndUsageInput>,
) -> aws_sdk_costexplorer::Client {
    let mut seq = mock!(aws_sdk_costexplorer::Client::get_cost_and_usage)
        .match_requests(rec(log))
        .sequence();
    for o in outputs {
        seq = seq.output(move || o.clone());
    }
    let rule = seq.build();
    mock_client!(aws_sdk_costexplorer, RuleMode::MatchAny, &[&rule])
}

fn one(days: Vec<ResultByTime>) -> GetCostAndUsageOutput {
    let mut b = GetCostAndUsageOutput::builder();
    for d in days {
        b = b.results_by_time(d);
    }
    b.build()
}

#[test]
fn category_for_tags_maps_role_agent_tags_to_ledger_categories() {
    assert_eq!(
        category_for_tags("agent_runner", "a1"),
        SpendCategory::AgentRunner
    );
    assert_eq!(category_for_tags("bench", ""), SpendCategory::BenchRunner);
    assert_eq!(
        category_for_tags("experiment", "a1"),
        SpendCategory::Experiment
    );
    assert_eq!(category_for_tags("", "a1"), SpendCategory::Experiment); // agent-launched: opt + agent tags only
    assert_eq!(category_for_tags("", ""), SpendCategory::StorageEgress);
    assert_eq!(
        category_for_tags("mystery", ""),
        SpendCategory::StorageEgress
    );
}

#[tokio::test]
async fn sends_a_daily_unblended_cost_query_filtered_by_ratchet_opt_and_grouped_by_role_and_agent_tags()
 {
    let log = Log::new();
    let ce = AwsCostReconciler::new(client_returning(vec![one(vec![])], &log));
    ce.actuals(&query()).await.unwrap();
    let input = log.first();
    let tp = input.time_period().unwrap();
    // end is exclusive: the day after the window's last day
    assert_eq!((tp.start(), tp.end()), ("2026-10-28", "2026-10-30"));
    assert_eq!(input.granularity(), Some(&Granularity::Daily));
    assert_eq!(input.metrics(), ["UnblendedCost"]);
    let tags = input.filter().unwrap().tags().unwrap();
    assert_eq!(
        (tags.key(), tags.values(), tags.match_options()),
        (
            Some("ratchet:opt"),
            ["opt-1".to_string()].as_slice(),
            [MatchOption::Equals].as_slice()
        )
    );
    let groups: Vec<_> = input
        .group_by()
        .iter()
        .map(|g| (g.r#type().cloned(), g.key().map(str::to_string)))
        .collect();
    assert_eq!(
        groups,
        [
            (
                Some(GroupDefinitionType::Tag),
                Some("ratchet:role".to_string())
            ),
            (
                Some(GroupDefinitionType::Tag),
                Some("ratchet:agent".to_string())
            )
        ]
    );
}

#[tokio::test]
async fn can_additionally_require_the_sandbox_tag() {
    let log = Log::new();
    let ce =
        AwsCostReconciler::new(client_returning(vec![one(vec![])], &log)).sandbox_id("rt-test");
    ce.actuals(&query()).await.unwrap();
    let input = log.first();
    let and = input.filter().unwrap().and();
    assert_eq!(and.len(), 2);
    let key_value = |e: &aws_sdk_costexplorer::types::Expression| {
        let t = e.tags().unwrap();
        (
            t.key().unwrap().to_string(),
            t.values().to_vec(),
            t.match_options().to_vec(),
        )
    };
    assert_eq!(
        key_value(&and[0]),
        (
            "ratchet:opt".to_string(),
            vec!["opt-1".to_string()],
            vec![MatchOption::Equals]
        )
    );
    assert_eq!(
        key_value(&and[1]),
        (
            "ratchet:sandbox".to_string(),
            vec!["rt-test".to_string()],
            vec![MatchOption::Equals]
        )
    );
}

#[tokio::test]
async fn sums_days_and_pages_into_ledger_categories() {
    let log = Log::new();
    let first = GetCostAndUsageOutput::builder()
        .results_by_time(day(
            "2026-10-28",
            vec![
                group("agent_runner", "a1", "0.50"),
                group("bench", "", "0.25"),
                group("", "a1", "0.125"),
            ],
            false,
        ))
        .next_page_token("p2")
        .build();
    let second = one(vec![day(
        "2026-10-29",
        vec![
            group("agent_runner", "a2", "0.25"),
            group("", "", "0.0625"),
            group("bench", "", "0"),
        ],
        false,
    )]);
    let r = AwsCostReconciler::new(client_returning(vec![first, second], &log))
        .actuals(&query())
        .await
        .unwrap();
    assert_eq!(
        r.by_category.into_iter().collect::<Vec<_>>(),
        [
            (SpendCategory::AgentRunner, 0.75),
            (SpendCategory::Experiment, 0.125),
            (SpendCategory::BenchRunner, 0.25),
            (SpendCategory::StorageEgress, 0.0625)
        ]
    );
    assert_eq!(r.groups, 5); // the zero-cost group is not counted
    assert!(r.complete);
    assert_eq!(r.note, None);
    let calls = log.all();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].next_page_token(), Some("p2"));
}

#[tokio::test]
async fn reports_incomplete_data_while_days_are_flagged_estimated_unless_accept_estimated() {
    let out = || {
        one(vec![
            day("2026-10-28", vec![group("bench", "", "1")], false),
            day("2026-10-29", vec![group("bench", "", "1")], true),
        ])
    };
    let r = AwsCostReconciler::new(client_returning(vec![out()], &Log::new()))
        .actuals(&query())
        .await
        .unwrap();
    assert!(!r.complete);
    assert!(
        r.note
            .as_deref()
            .unwrap()
            .contains("1 day(s) as estimated (2026-10-29)"),
        "{:?}",
        r.note
    );
    let accepted = AwsCostReconciler::new(client_returning(vec![out()], &Log::new()))
        .accept_estimated(true)
        .actuals(&query())
        .await
        .unwrap();
    assert!(accepted.complete);
}

#[tokio::test]
async fn explains_empty_results_tags_not_activated_data_not_there_yet() {
    let r = AwsCostReconciler::new(client_returning(
        vec![one(vec![day("2026-10-28", vec![], false)])],
        &Log::new(),
    ))
    .actuals(&query())
    .await
    .unwrap();
    assert_eq!((r.groups, r.by_category.len(), r.complete), (0, 0, true));
    assert!(r.note.unwrap().contains("cost-allocation tags"));
}

#[tokio::test]
async fn rejects_bad_ids_inverted_windows_non_numeric_amounts_and_surfaces_sdk_errors() {
    // No rules: a request for the invalid queries would panic the mock client.
    let none = || {
        mock_client!(
            aws_sdk_costexplorer,
            RuleMode::MatchAny,
            &[] as &[&aws_smithy_mocks::Rule]
        )
    };
    let mut q = query();
    q.optimization_id = "a*b".into();
    assert!(
        AwsCostReconciler::new(none())
            .actuals(&q)
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid optimization id")
    );
    let mut q = query();
    q.end_ms = q.start_ms - 1;
    assert!(
        AwsCostReconciler::new(none())
            .actuals(&q)
            .await
            .unwrap_err()
            .to_string()
            .contains("ends before")
    );
    assert!(
        AwsCostReconciler::new(none())
            .sandbox_id("A*")
            .actuals(&query())
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid sandbox id")
    );

    let nan = client_returning(
        vec![one(vec![day(
            "2026-10-28",
            vec![group("bench", "", "NaN")],
            false,
        )])],
        &Log::new(),
    );
    assert!(
        AwsCostReconciler::new(nan)
            .actuals(&query())
            .await
            .unwrap_err()
            .to_string()
            .contains("non-numeric")
    );

    let denied = mock!(aws_sdk_costexplorer::Client::get_cost_and_usage).then_error(|| {
        GetCostAndUsageError::generic(meta(
            "AccessDeniedException",
            "not authorized to perform: ce:GetCostAndUsage",
        ))
    });
    let client = mock_client!(aws_sdk_costexplorer, RuleMode::MatchAny, &[&denied]);
    assert!(
        AwsCostReconciler::new(client)
            .actuals(&query())
            .await
            .unwrap_err()
            .to_string()
            .contains("ce:GetCostAndUsage")
    );
}

#[tokio::test]
async fn an_unparsable_amount_is_an_error_not_a_zero() {
    for bad in ["abc", "Infinity", "-inf"] {
        let c = client_returning(
            vec![one(vec![day(
                "2026-10-28",
                vec![group("bench", "", bad)],
                false,
            )])],
            &Log::new(),
        );
        assert!(
            AwsCostReconciler::new(c).actuals(&query()).await.is_err(),
            "{bad}"
        );
    }
}

// ---- MockCostReconciler ---------------------------------------------------------------------------------

#[tokio::test]
async fn mock_returns_canned_or_computed_actuals_and_records_queries() {
    let m = MockCostReconciler::new().with_actuals(billed(&[(SpendCategory::BenchRunner, 2.0)]));
    let got = m.actuals(&query()).await.unwrap();
    assert_eq!(
        (
            got.by_category[&SpendCategory::BenchRunner],
            got.groups,
            got.complete,
            got.note.clone()
        ),
        (2.0, 1, true, None)
    );
    assert_eq!(m.calls(), [query()]);

    let default = MockCostReconciler::new().actuals(&query()).await.unwrap();
    assert!(
        default.by_category.is_empty()
            && default.groups == 0
            && default.complete
            && default.note.is_none()
    );

    let f = MockCostReconciler::new().with_actuals_fn(|q| {
        billed(&[(SpendCategory::Experiment, (q.end_ms - q.start_ms) as f64)])
    });
    assert_eq!(
        f.actuals(&query()).await.unwrap().by_category[&SpendCategory::Experiment],
        (DAY / 2) as f64
    );

    let e = MockCostReconciler::new()
        .with_error("boom")
        .actuals(&query())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("boom"));
}
