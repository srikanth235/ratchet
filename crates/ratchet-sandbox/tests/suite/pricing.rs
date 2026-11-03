//! Port of `packages/aws/src/pricing.test.ts`.

use std::sync::Arc;

use crate::common::{Log, meta, rec};
use aws_sdk_pricing::operation::get_products::{
    GetProductsError, GetProductsInput, GetProductsOutput,
};
use aws_sdk_pricing::types::FilterType;
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use ratchet_core::ports::PriceBook;
use ratchet_core::ports::mock::ManualClock;
use ratchet_sandbox::pricing::{AwsPriceBook, StaticPriceBook, parse_on_demand_usd};

fn item(usd: &str, unit: &str) -> String {
    serde_json::json!({ "terms": { "OnDemand": { "SKU.TERM": { "priceDimensions": { "SKU.TERM.RATE": { "unit": unit, "pricePerUnit": { "USD": usd } } } } } } })
        .to_string()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} !~ {b}");
}

#[tokio::test]
async fn static_knows_common_c7i_m7i_types_and_supports_overrides() {
    let p = StaticPriceBook::with(&[("c7i.large", 1.0)]);
    close(p.hourly_usd("c7i.2xlarge").await.unwrap(), 0.357);
    close(p.hourly_usd("m7i.xlarge").await.unwrap(), 0.2016);
    assert_eq!(p.hourly_usd("c7i.large").await.unwrap(), 1.0);
    assert!(
        p.hourly_usd("z9.mega")
            .await
            .unwrap_err()
            .to_string()
            .contains("no static price")
    );
}

#[tokio::test]
async fn static_prices_scale_linearly_with_size_for_c7i() {
    let p = StaticPriceBook::default();
    close(
        p.hourly_usd("c7i.4xlarge").await.unwrap() / p.hourly_usd("c7i.large").await.unwrap(),
        8.0,
    );
}

fn book(client: aws_sdk_pricing::Client, clock: Arc<ManualClock>, fallback: bool) -> AwsPriceBook {
    let mut b = AwsPriceBook::builder("us-east-1", client)
        .clock(clock)
        .ttl_ms(1000);
    if fallback {
        b = b.fallback(Arc::new(StaticPriceBook::default()));
    }
    b.build()
}

fn products(items: Vec<String>) -> GetProductsOutput {
    GetProductsOutput::builder()
        .set_price_list(Some(items))
        .build()
}

#[tokio::test]
async fn queries_the_pricing_api_with_exact_linux_shared_on_demand_filters() {
    let log: Log<GetProductsInput> = Log::new();
    let rule = mock!(aws_sdk_pricing::Client::get_products)
        .match_requests(rec(&log))
        .then_output(|| products(vec![item("0.3570000000", "Hrs")]));
    let client = mock_client!(aws_sdk_pricing, RuleMode::MatchAny, &[&rule]);
    let b = book(client, Arc::new(ManualClock::new(0)), false);
    close(b.hourly_usd("c7i.2xlarge").await.unwrap(), 0.357);
    let input = log.first();
    assert_eq!(input.service_code(), Some("AmazonEC2"));
    let filters: Vec<_> = input
        .filters()
        .iter()
        .map(|f| {
            (
                f.r#type().clone(),
                f.field().to_string(),
                f.value().to_string(),
            )
        })
        .collect();
    let term =
        |field: &str, value: &str| (FilterType::TermMatch, field.to_string(), value.to_string());
    assert_eq!(
        filters,
        [
            term("instanceType", "c7i.2xlarge"),
            term("regionCode", "us-east-1"),
            term("operatingSystem", "Linux"),
            term("tenancy", "Shared"),
            term("preInstalledSw", "NA"),
            term("capacitystatus", "Used"),
        ]
    );
}

#[tokio::test]
async fn caches_within_ttl_refetches_after_and_dedupes_concurrent_lookups() {
    let log: Log<GetProductsInput> = Log::new();
    let rule = mock!(aws_sdk_pricing::Client::get_products)
        .match_requests(rec(&log))
        .then_output(|| products(vec![item("0.5", "Hrs")]));
    let client = mock_client!(aws_sdk_pricing, RuleMode::MatchAny, &[&rule]);
    let clock = Arc::new(ManualClock::new(0));
    let b = book(client, clock.clone(), false);
    let (x, y) = tokio::join!(b.hourly_usd("m7i.large"), b.hourly_usd("m7i.large"));
    assert_eq!((x.unwrap(), y.unwrap()), (0.5, 0.5));
    b.hourly_usd("m7i.large").await.unwrap();
    assert_eq!(log.len(), 1);
    clock.advance(1001);
    b.hourly_usd("m7i.large").await.unwrap();
    assert_eq!(log.len(), 2);
}

#[tokio::test]
async fn errors_when_nothing_matches_or_falls_back_when_configured() {
    let empty = mock!(aws_sdk_pricing::Client::get_products).then_output(|| products(vec![]));
    let client = mock_client!(aws_sdk_pricing, RuleMode::MatchAny, &[&empty]);
    let e = book(client, Arc::new(ManualClock::new(0)), false)
        .hourly_usd("c7i.large")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no on-demand price"), "{e}");

    let empty = mock!(aws_sdk_pricing::Client::get_products).then_output(|| products(vec![]));
    let client = mock_client!(aws_sdk_pricing, RuleMode::MatchAny, &[&empty]);
    close(
        book(client, Arc::new(ManualClock::new(0)), true)
            .hourly_usd("c7i.large")
            .await
            .unwrap(),
        0.08925,
    );

    let offline = mock!(aws_sdk_pricing::Client::get_products)
        .then_error(|| GetProductsError::generic(meta("ServiceUnavailable", "offline")));
    let client = mock_client!(aws_sdk_pricing, RuleMode::MatchAny, &[&offline]);
    close(
        book(client, Arc::new(ManualClock::new(0)), true)
            .hourly_usd("c7i.large")
            .await
            .unwrap(),
        0.08925,
    );
}

#[test]
fn parse_on_demand_usd_ignores_non_hourly_and_zero_dimensions() {
    assert_eq!(
        parse_on_demand_usd(&item("1.25", "Hrs")).unwrap(),
        Some(1.25)
    );
    assert_eq!(parse_on_demand_usd(&item("1.25", "GB")).unwrap(), None);
    assert_eq!(
        parse_on_demand_usd(&item("0.0000000000", "Hrs")).unwrap(),
        None
    );
    assert_eq!(parse_on_demand_usd("{}").unwrap(), None);
    assert!(parse_on_demand_usd("not json").is_err());
}
