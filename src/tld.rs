#[path = "../data/tld_popularity.rs"]
mod tld_popularity;

use crate::error::AppError;
use crate::resolve::{
    resolve_domains_raw, ResolveConfig, ResolveResponse, RCODE_NOERROR, RCODE_NXDOMAIN,
};
use csv::{ReaderBuilder, StringRecord, Trim};
use rand::random;
use reqwest::{Client, StatusCode};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::time::Duration;

pub const IANA_TLD_SOURCE_URL: &str = "https://data.iana.org/TLD/tlds-alpha-by-domain.txt";
pub const ICANN_REGISTRY_AGREEMENTS_URL: &str =
    "https://www.icann.org/en/registry-agreements/csvdownload";
pub const ICANN_REGISTRY_AGREEMENTS_FALLBACK_URL: &str =
    "https://raw.githubusercontent.com/case/iana-data/main/data/source/icann-registry-agreement-table.csv";
pub const DEFAULT_TLD_SOURCE_URL: &str = IANA_TLD_SOURCE_URL;

const INFRASTRUCTURE_TLDS: &[&str] = &["arpa"];

/// TLDs that are not open for public registration.
///
/// - `edu`, `gov`, `int`, `mil`: IANA-sponsored TLDs without an ICANN registry
///   agreement.  Source: cross-reference of the IANA Root Zone Database
///   (`type == "sponsored"`) with the ICANN Registry Agreements CSV (no active
///   entry).
/// - `va`: .va (Vatican City) is reserved exclusively for the Holy See and is
///   not available through any public registrar.
const RESTRICTED_TLDS: &[&str] = &["edu", "gov", "int", "mil", "va"];

const ICANN_ACTIVE_STATUS: &str = "active";
const ICANN_BRAND_AGREEMENT_TYPE: &str = "brand (spec 13)";
const HTTP_FETCH_ATTEMPTS: usize = 4;
const HTTP_FETCH_RETRY_BASE_DELAY_MS: u64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TldLenRange {
    pub min: usize,
    pub max: Option<usize>,
}

impl TldLenRange {
    pub fn parse(input: &str) -> Result<Self, AppError> {
        if let Some((left, right)) = input.split_once("..") {
            let min = if left.is_empty() {
                1
            } else {
                parse_positive(left)?
            };
            let max = if right.is_empty() {
                None
            } else {
                Some(parse_positive(right)?)
            };

            if let Some(max) = max {
                if min > max {
                    return Err(AppError::invalid_tld_len(input));
                }
            }

            Ok(Self { min, max })
        } else {
            let exact = parse_positive(input)?;
            Ok(Self {
                min: exact,
                max: Some(exact),
            })
        }
    }

    pub fn contains(&self, len: usize) -> bool {
        if len < self.min {
            return false;
        }

        match self.max {
            Some(max) => len <= max,
            None => true,
        }
    }
}

fn parse_positive(input: &str) -> Result<usize, AppError> {
    match input.parse::<usize>() {
        Ok(0) | Err(_) => Err(AppError::invalid_tld_len(input)),
        Ok(value) => Ok(value),
    }
}

pub fn filter_tlds(
    tlds: &[String],
    prefix: Option<&str>,
    range: Option<TldLenRange>,
) -> Vec<String> {
    let mut filtered = tlds
        .iter()
        .filter(|tld| match prefix {
            Some(prefix) => tld.starts_with(prefix),
            None => true,
        })
        .filter(|tld| match range {
            Some(range) => range.contains(tld.len()),
            None => true,
        })
        .cloned()
        .collect::<Vec<_>>();

    sort_tlds(&mut filtered);
    filtered
}

pub fn sort_tlds(tlds: &mut [String]) {
    tlds.sort_by(|left, right| compare_tlds(left, right));
}

fn compare_tlds(left: &str, right: &str) -> Ordering {
    left.len()
        .cmp(&right.len())
        .then_with(|| compare_popularity(left, right))
        .then_with(|| left.cmp(right))
}

fn compare_popularity(left: &str, right: &str) -> Ordering {
    match (popularity_index(left), popularity_index(right)) {
        (Some(left_index), Some(right_index)) => left_index.cmp(&right_index),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn popularity_index(tld: &str) -> Option<usize> {
    tld_popularity::TLD_POPULARITY
        .iter()
        .position(|entry| *entry == tld)
}

pub fn pinned_index(tld: &str) -> Option<usize> {
    tld_popularity::PINNED_TLDS
        .iter()
        .position(|entry| *entry == tld)
}

pub fn is_pinned(tld: &str) -> bool {
    pinned_index(tld).is_some()
}

pub fn split_groups(tlds: &[String], group_size: usize) -> Vec<Vec<String>> {
    let size = group_size.max(1);
    tlds.chunks(size).map(|chunk| chunk.to_vec()).collect()
}

pub async fn fetch_filtered_tlds(
    client: &Client,
    resolve_config: &ResolveConfig,
    source_url: &str,
) -> Result<Vec<String>, AppError> {
    let excluded_tlds = fetch_icann_excluded_tlds(client).await?;
    let candidates = fetch_candidates_from_iana(client, source_url)
        .await?
        .into_iter()
        .filter(|tld| !excluded_tlds.contains(tld))
        .collect::<Vec<_>>();

    let nic_queries = candidates
        .iter()
        .map(|tld| format!("nic.{tld}"))
        .collect::<Vec<_>>();
    let nic_results = resolve_raw_async(resolve_config.clone(), nic_queries).await?;

    let active_candidates = candidates
        .into_iter()
        .zip(nic_results.into_iter())
        .filter_map(|(tld, response)| match response {
            Some(ResolveResponse {
                rcode: RCODE_NOERROR,
                answer_count,
            }) if answer_count > 0 => Some(tld),
            _ => None,
        })
        .collect::<Vec<_>>();

    let probe_queries = active_candidates
        .iter()
        .map(|tld| format!("xyzzy-probe-test-{:08x}.{tld}", random::<u32>()))
        .collect::<Vec<_>>();
    let probe_results = resolve_raw_async(resolve_config.clone(), probe_queries).await?;

    let mut included = active_candidates
        .into_iter()
        .zip(probe_results.into_iter())
        .filter_map(|(tld, response)| match response {
            Some(ResolveResponse {
                rcode: RCODE_NXDOMAIN,
                ..
            }) => Some(tld),
            _ => None,
        })
        .collect::<Vec<_>>();

    sort_tlds(&mut included);
    Ok(included)
}

async fn fetch_candidates_from_iana(
    client: &Client,
    source_url: &str,
) -> Result<Vec<String>, AppError> {
    let text = fetch_text_with_retry(client, source_url, None).await?;
    parse_iana_candidates(&text)
}

async fn fetch_icann_excluded_tlds(client: &Client) -> Result<HashSet<String>, AppError> {
    fetch_icann_excluded_tlds_with_sources(
        client,
        ICANN_REGISTRY_AGREEMENTS_URL,
        ICANN_REGISTRY_AGREEMENTS_FALLBACK_URL,
    )
    .await
}

async fn fetch_icann_excluded_tlds_with_sources(
    client: &Client,
    primary_url: &str,
    fallback_url: &str,
) -> Result<HashSet<String>, AppError> {
    let text = match fetch_text_with_retry(client, primary_url, Some(Duration::from_secs(30))).await
    {
        Ok(text) => text,
        Err(primary_err) => {
            eprintln!(
                "warning: primary ICANN registry agreements source failed; falling back to mirror"
            );
            eprintln!("  --> {primary_url}");
            eprintln!("  --> {fallback_url}");
            eprintln!("  = detail: {}", primary_err.to_string().trim_end());
            fetch_text_with_retry(client, fallback_url, Some(Duration::from_secs(30))).await?
        }
    };

    parse_icann_excluded_tlds(&text)
}

async fn fetch_text_with_retry(
    client: &Client,
    url: &str,
    timeout: Option<Duration>,
) -> Result<String, AppError> {
    let mut last_error = None;

    for attempt in 0..HTTP_FETCH_ATTEMPTS {
        let request = client.get(url);
        let request = if let Some(timeout) = timeout {
            request.timeout(timeout)
        } else {
            request
        };

        match request.send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    return response.text().await.map_err(AppError::network_request);
                }

                let message = format!("unexpected HTTP status {status} from {url}");
                if !is_retryable_status(status) || attempt + 1 == HTTP_FETCH_ATTEMPTS {
                    return Err(AppError::network_request(message));
                }

                last_error = Some(message);
            }
            Err(err) => {
                if attempt + 1 == HTTP_FETCH_ATTEMPTS {
                    return Err(AppError::network_request(err));
                }

                last_error = Some(err.to_string());
            }
        }

        if let Some(last_error) = &last_error {
            eprintln!(
                "warning: request attempt {}/{} failed for {url}: {last_error}",
                attempt + 1,
                HTTP_FETCH_ATTEMPTS
            );
        }
        tokio::time::sleep(http_fetch_retry_delay(attempt)).await;
    }

    Err(AppError::network_request(
        last_error.unwrap_or_else(|| format!("request to {url} failed")),
    ))
}

fn is_retryable_status(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS
}

fn http_fetch_retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(HTTP_FETCH_RETRY_BASE_DELAY_MS * (1_u64 << attempt))
}

fn parse_iana_candidates(text: &str) -> Result<Vec<String>, AppError> {
    let candidates = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| line.to_ascii_lowercase())
        .filter(|tld| tld.chars().all(|ch| ch.is_ascii_lowercase()))
        .filter(|tld| !INFRASTRUCTURE_TLDS.contains(&tld.as_str()))
        .filter(|tld| !RESTRICTED_TLDS.contains(&tld.as_str()))
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        return Err(AppError::new("failed to parse TLD list response")
            .with_help("the TLD source did not return the expected IANA text format"));
    }

    Ok(candidates)
}

fn parse_icann_excluded_tlds(text: &str) -> Result<HashSet<String>, AppError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut reader = ReaderBuilder::new()
        .trim(Trim::All)
        .from_reader(text.as_bytes());
    let headers = reader.headers().map_err(icann_csv_parse_error)?.clone();
    let tld_index = find_csv_header(&headers, "Top Level Domain")?;
    let agreement_type_index = find_csv_header(&headers, "Agreement Type")?;
    let status_index = find_csv_header(&headers, "Agreement Status")?;

    let mut excluded = HashSet::new();
    for record in reader.records() {
        let record = record.map_err(icann_csv_parse_error)?;
        let tld = record
            .get(tld_index)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !tld.chars().all(|ch| ch.is_ascii_lowercase()) {
            continue;
        }

        let agreement_types = record
            .get(agreement_type_index)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let status = record
            .get(status_index)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let is_brand = agreement_types
            .split(',')
            .any(|entry| entry.trim() == ICANN_BRAND_AGREEMENT_TYPE);
        let is_inactive = status != ICANN_ACTIVE_STATUS;

        if is_brand || is_inactive {
            excluded.insert(tld);
        }
    }

    Ok(excluded)
}

fn find_csv_header(headers: &StringRecord, expected: &str) -> Result<usize, AppError> {
    headers
        .iter()
        .position(|header| header.trim_matches('"').trim() == expected)
        .ok_or_else(|| {
            AppError::new("failed to parse ICANN registry agreements response")
                .with_help(format!("missing expected CSV column '{expected}'"))
        })
}

fn icann_csv_parse_error(err: csv::Error) -> AppError {
    AppError::new(format!(
        "failed to parse ICANN registry agreements response: {err}"
    ))
    .with_help("the ICANN registry agreements endpoint did not return the expected CSV format")
}

async fn resolve_raw_async(
    resolve_config: ResolveConfig,
    domains: Vec<String>,
) -> Result<Vec<Option<ResolveResponse>>, AppError> {
    tokio::task::spawn_blocking(move || resolve_domains_raw(&resolve_config, &domains))
        .await
        .map_err(|err| AppError::new(format!("DNS worker task failed: {err}")))?
}

#[cfg(test)]
mod tests {
    use super::{
        fetch_icann_excluded_tlds_with_sources, fetch_text_with_retry, filter_tlds, is_pinned,
        parse_iana_candidates, parse_icann_excluded_tlds, pinned_index, TldLenRange,
    };
    use crate::http::build_http_client_with_timeouts;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    struct FlakyResponder {
        failures_before_success: usize,
        attempts: Arc<AtomicUsize>,
        success_body: &'static str,
    }

    impl Respond for FlakyResponder {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            let attempt = self.attempts.fetch_add(1, AtomicOrdering::SeqCst);
            if attempt < self.failures_before_success {
                ResponseTemplate::new(500)
            } else {
                ResponseTemplate::new(200).set_body_string(self.success_body)
            }
        }
    }

    #[test]
    fn parses_supported_length_ranges() {
        assert_eq!(
            TldLenRange::parse("2").unwrap(),
            TldLenRange {
                min: 2,
                max: Some(2)
            }
        );
        assert_eq!(
            TldLenRange::parse("2..5").unwrap(),
            TldLenRange {
                min: 2,
                max: Some(5)
            }
        );
        assert_eq!(
            TldLenRange::parse("..3").unwrap(),
            TldLenRange {
                min: 1,
                max: Some(3)
            }
        );
        assert_eq!(
            TldLenRange::parse("4..").unwrap(),
            TldLenRange { min: 4, max: None }
        );
    }

    #[test]
    fn sorts_by_length_then_popularity_then_alphabetically() {
        let tlds = vec![
            "shop".to_string(),
            "xyz".to_string(),
            "ai".to_string(),
            "com".to_string(),
            "co".to_string(),
            "app".to_string(),
        ];

        let filtered = filter_tlds(&tlds, None, None);
        assert_eq!(filtered, vec!["ai", "co", "com", "app", "xyz", "shop"]);
    }

    #[test]
    fn reports_pinned_tlds() {
        assert_eq!(pinned_index("com"), Some(0));
        assert_eq!(pinned_index("dev"), Some(9));
        assert!(is_pinned("shop"));
        assert!(!is_pinned("info"));
    }

    #[test]
    fn parses_iana_candidates_and_filters_infrastructure_idn_and_restricted() {
        let text = "# Version 2026041400\nCOM\nDEV\nARPA\nXN--P1AI\nIO\nEDU\nGOV\nMIL\nINT\nVA\n";

        let candidates = parse_iana_candidates(text).unwrap();
        assert_eq!(candidates, vec!["com", "dev", "io"]);
    }

    #[test]
    fn parses_icann_csv_and_excludes_brand_and_inactive_tlds() {
        let csv = concat!(
            "\u{feff}\"Top Level Domain\",\"Agreement Type\",\"Agreement Status\"\n",
            "\"google\",\"Base, Brand (Spec 13), Non-Sponsored\",\"active\"\n",
            "\"com\",\"Base, Non-Sponsored\",\"active\"\n",
            "\"abarth\",\"Base, Brand (Spec 13), Non-Sponsored\",\"terminated\"\n",
            "\"doosan\",\"Base, Non-Sponsored\",\"terminated\"\n",
            "\"xn--fiqs8s\",\"Base, Brand (Spec 13), Non-Sponsored\",\"active\"\n",
        );

        let excluded = parse_icann_excluded_tlds(csv).unwrap();
        assert_eq!(
            excluded,
            HashSet::from([
                "google".to_string(),
                "abarth".to_string(),
                "doosan".to_string(),
            ])
        );
        assert!(!excluded.contains("com"));
    }

    #[tokio::test]
    async fn retries_transient_server_errors_when_fetching_text() {
        let server = MockServer::start().await;
        let attempts = Arc::new(AtomicUsize::new(0));

        Mock::given(method("GET"))
            .and(path("/icann.csv"))
            .respond_with(FlakyResponder {
                failures_before_success: 2,
                attempts: attempts.clone(),
                success_body: "ok",
            })
            .mount(&server)
            .await;

        let client =
            build_http_client_with_timeouts(false, Duration::from_secs(1), Duration::from_secs(2))
                .unwrap();

        let body = fetch_text_with_retry(&client, &format!("{}/icann.csv", server.uri()), None)
            .await
            .unwrap();

        assert_eq!(body, "ok");
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 3);
    }

    #[tokio::test]
    async fn falls_back_to_mirrored_icann_csv_when_primary_fails() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/primary.csv"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let csv = concat!(
            "\u{feff}\"Top Level Domain\",\"Agreement Type\",\"Agreement Status\"\n",
            "\"google\",\"Base, Brand (Spec 13), Non-Sponsored\",\"active\"\n",
            "\"com\",\"Base, Non-Sponsored\",\"active\"\n",
        );
        Mock::given(method("GET"))
            .and(path("/fallback.csv"))
            .respond_with(ResponseTemplate::new(200).set_body_string(csv))
            .mount(&server)
            .await;

        let client =
            build_http_client_with_timeouts(false, Duration::from_secs(1), Duration::from_secs(2))
                .unwrap();

        let excluded = fetch_icann_excluded_tlds_with_sources(
            &client,
            &format!("{}/primary.csv", server.uri()),
            &format!("{}/fallback.csv", server.uri()),
        )
        .await
        .unwrap();

        assert!(excluded.contains("google"));
        assert!(!excluded.contains("com"));
    }
}
