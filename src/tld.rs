#[path = "../data/tld_popularity.rs"]
mod tld_popularity;

use crate::error::AppError;
use crate::resolve::{
    resolve_domains_raw, ResolveConfig, ResolveResponse, RCODE_NOERROR, RCODE_NXDOMAIN,
};
use rand::random;
use reqwest::Client;
use serde_json::Value;
use std::cmp::Ordering;

pub const DEFAULT_TLD_SOURCE_URL: &str = "https://tld-list.com/df/tld-list-details.json";

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

pub fn split_groups(tlds: &[String], group_size: usize) -> Vec<Vec<String>> {
    let size = group_size.max(1);
    tlds.chunks(size).map(|chunk| chunk.to_vec()).collect()
}

pub async fn fetch_filtered_tlds(
    client: &Client,
    resolve_config: &ResolveConfig,
    source_url: &str,
) -> Result<Vec<String>, AppError> {
    let response = client
        .get(source_url)
        .send()
        .await
        .map_err(AppError::network_request)?;

    if !response.status().is_success() {
        return Err(AppError::network_request(format!(
            "unexpected HTTP status {} from {source_url}",
            response.status()
        )));
    }

    let payload = response
        .json::<Value>()
        .await
        .map_err(AppError::network_request)?;
    let object = payload.as_object().ok_or_else(|| {
        AppError::new("failed to parse TLD list response")
            .with_help("the TLD source did not return the expected JSON object")
    })?;

    let candidates = object
        .iter()
        .filter_map(|(tld, details)| {
            if !tld.chars().all(|ch| ch.is_ascii_lowercase()) {
                return None;
            }

            let punycode = details.get("punycode");
            if punycode.is_some() && !punycode.is_some_and(Value::is_null) {
                return None;
            }

            let kind = details
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if kind == "infrastructure" {
                return None;
            }

            Some(tld.clone())
        })
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
    use super::{filter_tlds, TldLenRange};

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
}
