//! Instance prices: a static table for simulation and as a fallback, and the AWS Pricing API. (TS `pricing.ts`)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use aws_sdk_pricing::types::{Filter, FilterType};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};

use crate::error::{Error, Result, sdk};
use ratchet_core::ports::mock::SystemClock;
use ratchet_core::ports::{Clock, PortError, PortResult, PriceBook};

/// Approximate us-east-1 Linux on-demand USD/hour. For simulation and as a fallback; the live number comes from
/// [`AwsPriceBook`].
pub const STATIC_HOURLY_USD: &[(&str, f64)] = &[
    ("t3.medium", 0.0416),
    ("t3.large", 0.0832),
    ("t3.xlarge", 0.1664),
    ("c7i.large", 0.08925),
    ("c7i.xlarge", 0.1785),
    ("c7i.2xlarge", 0.357),
    ("c7i.4xlarge", 0.714),
    ("c7i.8xlarge", 1.428),
    ("c7i.12xlarge", 2.142),
    ("c7i.16xlarge", 2.856),
    ("c7i.24xlarge", 4.284),
    ("c7i.48xlarge", 8.568),
    ("m7i.large", 0.1008),
    ("m7i.xlarge", 0.2016),
    ("m7i.2xlarge", 0.4032),
    ("m7i.4xlarge", 0.8064),
    ("m7i.8xlarge", 1.6128),
    ("m7i.12xlarge", 2.4192),
    ("m7i.16xlarge", 3.2256),
    ("m7i.24xlarge", 4.8384),
    ("m7i.48xlarge", 9.6768),
];

#[derive(Debug, Clone)]
pub struct StaticPriceBook {
    table: HashMap<String, f64>,
}

impl Default for StaticPriceBook {
    fn default() -> Self {
        Self::new([])
    }
}

impl StaticPriceBook {
    /// The built-in table with `overrides` on top.
    pub fn new(overrides: impl IntoIterator<Item = (String, f64)>) -> Self {
        let mut table: HashMap<String, f64> = STATIC_HOURLY_USD
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect();
        table.extend(overrides);
        Self { table }
    }

    /// Convenience for tests: overrides from `(&str, f64)` pairs.
    pub fn with(overrides: &[(&str, f64)]) -> Self {
        Self::new(overrides.iter().map(|(k, v)| ((*k).to_string(), *v)))
    }
}

#[async_trait]
impl PriceBook for StaticPriceBook {
    async fn hourly_usd(&self, instance_type: &str) -> PortResult<f64> {
        self.table.get(instance_type).copied().ok_or_else(|| {
            PortError::failed(format!("no static price for instance type {instance_type}"))
        })
    }
}

type Inflight = Shared<BoxFuture<'static, std::result::Result<f64, Arc<Error>>>>;

struct Inner {
    client: aws_sdk_pricing::Client,
    region: String,
    clock: Arc<dyn Clock>,
    ttl_ms: i64,
    fallback: Option<Arc<dyn PriceBook>>,
    cache: Mutex<HashMap<String, (f64, i64)>>,
    inflight: Mutex<HashMap<String, Inflight>>,
}

/// On-demand prices from the AWS Pricing API (`GetProducts`), cached for `ttl` with concurrent lookups of one type
/// deduplicated. Falls back to `fallback` when the API fails or has no match (e.g. offline).
#[derive(Clone)]
pub struct AwsPriceBook {
    inner: Arc<Inner>,
}

/// Options for [`AwsPriceBook`].
pub struct AwsPriceBookBuilder {
    region: String,
    client: aws_sdk_pricing::Client,
    clock: Arc<dyn Clock>,
    ttl_ms: i64,
    fallback: Option<Arc<dyn PriceBook>>,
}

impl AwsPriceBookBuilder {
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Cache lifetime. Default 24 h.
    pub fn ttl_ms(mut self, ttl_ms: i64) -> Self {
        self.ttl_ms = ttl_ms;
        self
    }

    /// Used when the Pricing API fails or has no match (e.g. offline).
    pub fn fallback(mut self, fallback: Arc<dyn PriceBook>) -> Self {
        self.fallback = Some(fallback);
        self
    }

    pub fn build(self) -> AwsPriceBook {
        AwsPriceBook {
            inner: Arc::new(Inner {
                client: self.client,
                region: self.region,
                clock: self.clock,
                ttl_ms: self.ttl_ms,
                fallback: self.fallback,
                cache: Mutex::default(),
                inflight: Mutex::default(),
            }),
        }
    }
}

impl AwsPriceBook {
    /// `region` is the region whose prices to read, e.g. `us-east-1`. The Pricing API itself only exists in
    /// us-east-1 / ap-south-1 / eu-central-1: build `client` for one of those.
    pub fn builder(
        region: impl Into<String>,
        client: aws_sdk_pricing::Client,
    ) -> AwsPriceBookBuilder {
        AwsPriceBookBuilder {
            region: region.into(),
            client,
            clock: Arc::new(SystemClock),
            ttl_ms: 24 * 3_600_000,
            fallback: None,
        }
    }

    pub fn new(region: impl Into<String>, client: aws_sdk_pricing::Client) -> Self {
        Self::builder(region, client).build()
    }

    /// A price book on the default credential chain (or `profile`), Pricing API client in us-east-1.
    pub async fn connect(region: impl Into<String>, profile: Option<&str>) -> Self {
        let cfg = crate::compute::aws::load_sdk_config("us-east-1", profile).await;
        Self::new(region, aws_sdk_pricing::Client::new(&cfg))
    }
}

impl Inner {
    async fn fetch(&self, instance_type: &str) -> Result<f64> {
        let term = |field: &str, value: &str| {
            Filter::builder()
                .r#type(FilterType::TermMatch)
                .field(field)
                .value(value)
                .build()
                .map_err(|e| Error::other(e.to_string()))
        };
        let r = self
            .client
            .get_products()
            .service_code("AmazonEC2")
            .format_version("aws_v1")
            .max_results(10)
            .filters(term("instanceType", instance_type)?)
            .filters(term("regionCode", &self.region)?)
            .filters(term("operatingSystem", "Linux")?)
            .filters(term("tenancy", "Shared")?)
            .filters(term("preInstalledSw", "NA")?)
            .filters(term("capacitystatus", "Used")?)
            .send()
            .await
            .map_err(sdk)?;
        for item in r.price_list() {
            if let Some(usd) = parse_on_demand_usd(item)? {
                return Ok(usd);
            }
        }
        Err(Error::other(format!(
            "no on-demand price found for {instance_type} in {}",
            self.region
        )))
    }
}

fn share_error(e: &Error) -> Error {
    match e {
        Error::Aws { code, message } => Error::aws(code.clone(), message.clone()),
        other => Error::other(other.to_string()),
    }
}

#[async_trait]
impl PriceBook for AwsPriceBook {
    async fn hourly_usd(&self, instance_type: &str) -> PortResult<f64> {
        let inner = &self.inner;
        if let Some((usd, at)) = inner
            .cache
            .lock()
            .expect("price cache poisoned")
            .get(instance_type)
            .copied()
            && inner.clock.now() - at < inner.ttl_ms
        {
            return Ok(usd);
        }
        let fut = {
            let mut inflight = inner.inflight.lock().expect("price inflight poisoned");
            inflight
                .entry(instance_type.to_string())
                .or_insert_with(|| {
                    let inner = inner.clone();
                    let t = instance_type.to_string();
                    async move {
                        let r = match inner.fetch(&t).await {
                            Ok(usd) => {
                                inner
                                    .cache
                                    .lock()
                                    .expect("price cache poisoned")
                                    .insert(t.clone(), (usd, inner.clock.now()));
                                Ok(usd)
                            }
                            Err(e) => match &inner.fallback {
                                Some(fb) => fb
                                    .hourly_usd(&t)
                                    .await
                                    .map_err(|e| Arc::new(Error::from(e))),
                                None => Err(Arc::new(e)),
                            },
                        };
                        inner
                            .inflight
                            .lock()
                            .expect("price inflight poisoned")
                            .remove(&t);
                        r
                    }
                    .boxed()
                    .shared()
                })
                .clone()
        };
        fut.await.map_err(|e| PortError::from(share_error(&e)))
    }
}

/// Extract the hourly USD from one Pricing API `PriceList` entry (a JSON document).
pub fn parse_on_demand_usd(price_list_json: &str) -> Result<Option<f64>> {
    let doc: serde_json::Value = serde_json::from_str(price_list_json)?;
    let Some(on_demand) = doc.pointer("/terms/OnDemand").and_then(|v| v.as_object()) else {
        return Ok(None);
    };
    for term in on_demand.values() {
        let Some(dims) = term.get("priceDimensions").and_then(|v| v.as_object()) else {
            continue;
        };
        for dim in dims.values() {
            let usd = dim
                .pointer("/pricePerUnit/USD")
                .and_then(|v| v.as_str())
                .and_then(|s| s.trim().parse::<f64>().ok());
            if dim.get("unit").and_then(|v| v.as_str()) == Some("Hrs")
                && let Some(usd) = usd.filter(|u| u.is_finite() && *u > 0.0)
            {
                return Ok(Some(usd));
            }
        }
    }
    Ok(None)
}
