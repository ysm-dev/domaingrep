use crate::error::AppError;
use futures::future::join_all;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

pub const DEFAULT_PRIMARY_DOH_URL: &str = "https://cloudflare-dns.com/dns-query";
pub const DEFAULT_FALLBACK_DOH_URL: &str = "https://dns.google/resolve";
pub const DEFAULT_CONCURRENCY: usize = 100;

#[derive(Debug, Clone)]
pub struct DnsResolver {
    client: Client,
    primary_url: String,
    fallback_url: String,
    semaphore: Arc<Semaphore>,
}

#[derive(Debug, Clone)]
pub struct DnsBatchResult {
    pub results: Vec<DnsBatchEntry>,
    pub failures: usize,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct DnsBatchEntry {
    pub domain: String,
    pub available: Result<bool, DnsQueryError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsStatus {
    pub status: u16,
    pub answer_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsQueryError {
    Network(String),
}

impl Display for DnsQueryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for DnsQueryError {}

#[derive(Debug, Deserialize)]
struct DohResponse {
    #[serde(rename = "Status")]
    status: u16,
    #[serde(rename = "Answer")]
    answer: Option<Vec<serde_json::Value>>,
}

impl DnsResolver {
    pub fn new(
        client: Client,
        primary_url: impl Into<String>,
        fallback_url: impl Into<String>,
    ) -> Self {
        Self {
            client,
            primary_url: primary_url.into(),
            fallback_url: fallback_url.into(),
            semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
        }
    }

    pub fn with_concurrency(
        client: Client,
        primary_url: impl Into<String>,
        fallback_url: impl Into<String>,
        concurrency: usize,
    ) -> Self {
        Self {
            client,
            primary_url: primary_url.into(),
            fallback_url: fallback_url.into(),
            semaphore: Arc::new(Semaphore::new(concurrency.max(1))),
        }
    }

    pub async fn check_domains(&self, domains: &[String]) -> DnsBatchResult {
        let tasks = domains.iter().enumerate().map(|(index, domain)| {
            let resolver = self.clone();
            let domain = domain.clone();
            async move { (index, resolver.check_availability(&domain).await, domain) }
        });

        let mut entries = join_all(tasks).await;
        entries.sort_by_key(|(index, _, _)| *index);

        let mut failures = 0;
        let mut results = Vec::with_capacity(entries.len());
        for (_, result, domain) in entries {
            if result.is_err() {
                failures += 1;
            }
            results.push(DnsBatchEntry {
                domain,
                available: result,
            });
        }

        DnsBatchResult {
            results,
            failures,
            total: domains.len(),
        }
    }

    pub async fn check_availability(&self, domain: &str) -> Result<bool, DnsQueryError> {
        let status = self.query_status(domain).await?;
        Ok(status.status == 3)
    }

    pub async fn query_status(&self, domain: &str) -> Result<DnsStatus, DnsQueryError> {
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("dns semaphore should stay open");

        match self.request(&self.primary_url, domain).await {
            Ok(status) if matches!(status.status, 0 | 3) => Ok(status),
            Ok(_) | Err(_) => self.request(&self.fallback_url, domain).await,
        }
    }

    async fn request(&self, url: &str, domain: &str) -> Result<DnsStatus, DnsQueryError> {
        let response = self
            .client
            .get(url)
            .query(&[("name", domain), ("type", "NS")])
            .header("accept", "application/dns-json")
            .send()
            .await
            .map_err(|err| DnsQueryError::Network(err.to_string()))?;

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(DnsQueryError::Network("HTTP 429 rate limited".to_string()));
        }

        if !response.status().is_success() {
            return Err(DnsQueryError::Network(format!(
                "unexpected HTTP status {}",
                response.status()
            )));
        }

        let payload = response
            .json::<DohResponse>()
            .await
            .map_err(|err| DnsQueryError::Network(err.to_string()))?;

        Ok(DnsStatus {
            status: payload.status,
            answer_count: payload.answer.unwrap_or_default().len(),
        })
    }
}

pub fn build_http_client(force_http2: bool) -> Result<Client, AppError> {
    let builder = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .user_agent(format!("domaingrep/{}", env!("CARGO_PKG_VERSION")))
        .pool_max_idle_per_host(50);

    let builder = if force_http2 {
        builder.http2_prior_knowledge()
    } else {
        builder
    };

    builder.build().map_err(AppError::network_request)
}
