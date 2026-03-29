pub mod cache;
pub mod cli;
pub mod dns;
pub mod error;
pub mod hack;
pub mod input;
pub mod output;
pub mod tld;
pub mod update;

use cache::{CacheConfig, CacheStore};
use cli::Cli;
use dns::{build_http_client, DnsResolver, DEFAULT_FALLBACK_DOH_URL, DEFAULT_PRIMARY_DOH_URL};
use error::AppError;
use hack::HackTrie;
use input::{parse, InputMode};
use output::{visible_results, CheckMethod, DomainResult, OutputOptions, ResultKind};
use std::env;
use std::path::PathBuf;
use tld::TldLenRange;
use update::{take_if_finished, UpdateConfig};

pub use output::{
    CheckMethod as OutputCheckMethod, DomainResult as OutputDomainResult,
    ResultKind as OutputResultKind,
};

pub const DEFAULT_CACHE_ASSET_URL: &str =
    "https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.bin.gz";
pub const DEFAULT_CACHE_CHECKSUM_URL: &str =
    "https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.sha256";

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub cache_dir: PathBuf,
    pub cache_url: String,
    pub cache_checksum_url: String,
    pub update_api_url: String,
    pub doh_primary_url: String,
    pub doh_fallback_url: String,
    pub disable_update: bool,
}

#[derive(Debug, Clone)]
pub struct RunReport {
    pub stdout: String,
    pub stderr: Vec<String>,
    pub exit_code: i32,
}

#[derive(Debug, Default)]
struct ResolutionSummary {
    results: Vec<DomainResult>,
    dns_failures: usize,
    dns_total: usize,
}

impl RuntimeConfig {
    pub fn from_env() -> Result<Self, AppError> {
        let cache_dir = match env::var_os("DOMAINGREP_CACHE_DIR") {
            Some(path) => PathBuf::from(path),
            None => dirs::cache_dir()
                .map(|path| path.join("domaingrep"))
                .ok_or_else(AppError::cache_dir_unavailable)?,
        };

        Ok(Self {
            cache_dir,
            cache_url: env::var("DOMAINGREP_CACHE_URL")
                .unwrap_or_else(|_| DEFAULT_CACHE_ASSET_URL.to_string()),
            cache_checksum_url: env::var("DOMAINGREP_CACHE_CHECKSUM_URL")
                .unwrap_or_else(|_| DEFAULT_CACHE_CHECKSUM_URL.to_string()),
            update_api_url: env::var("DOMAINGREP_UPDATE_API_URL")
                .unwrap_or_else(|_| update::DEFAULT_UPDATE_API_URL.to_string()),
            doh_primary_url: env::var("DOMAINGREP_DOH_PRIMARY_URL")
                .unwrap_or_else(|_| DEFAULT_PRIMARY_DOH_URL.to_string()),
            doh_fallback_url: env::var("DOMAINGREP_DOH_FALLBACK_URL")
                .unwrap_or_else(|_| DEFAULT_FALLBACK_DOH_URL.to_string()),
            disable_update: env::var("DOMAINGREP_DISABLE_UPDATE")
                .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        })
    }
}

pub async fn run(cli: Cli, runtime: RuntimeConfig) -> Result<RunReport, AppError> {
    if matches!(cli.limit, Some(0)) {
        return Err(AppError::limit_must_be_at_least_one());
    }

    let raw_domain = cli.domain.clone().ok_or_else(AppError::no_domain)?;
    let input = parse(&raw_domain)?;
    let tld_range = cli.tld_len.as_deref().map(TldLenRange::parse).transpose()?;

    let force_http2 = runtime.cache_url.starts_with("https://")
        && runtime.cache_checksum_url.starts_with("https://")
        && runtime.update_api_url.starts_with("https://")
        && runtime.doh_primary_url.starts_with("https://")
        && runtime.doh_fallback_url.starts_with("https://");
    let client = build_http_client(force_http2)?;

    let update_handle = if runtime.disable_update {
        None
    } else {
        update::maybe_start(
            client.clone(),
            UpdateConfig {
                cache_dir: runtime.cache_dir.clone(),
                api_url: runtime.update_api_url.clone(),
                current_version: env!("CARGO_PKG_VERSION").to_string(),
            },
        )?
    };

    let cache = CacheStore::load_or_update(
        client.clone(),
        CacheConfig {
            cache_dir: runtime.cache_dir.clone(),
            asset_url: runtime.cache_url.clone(),
            checksum_url: runtime.cache_checksum_url.clone(),
        },
    )
    .await?;

    let regular_tlds = tld::filter_tlds(cache.tlds(), input.tld_prefix.as_deref(), tld_range);

    let resolver = DnsResolver::new(
        client,
        runtime.doh_primary_url.clone(),
        runtime.doh_fallback_url.clone(),
    );

    let mut all_results = Vec::new();
    let mut dns_failures = 0usize;
    let mut dns_total = 0usize;

    if input.mode == InputMode::SldOnly {
        let hack_tlds = tld::filter_tlds(cache.tlds(), None, tld_range);
        let trie = HackTrie::new(hack_tlds.iter().map(String::as_str));
        let summary = resolve_hacks(&input.sld, &trie, &cache, &resolver).await?;
        dns_failures += summary.dns_failures;
        dns_total += summary.dns_total;
        all_results.extend(summary.results);
    }

    let summary = resolve_regular(&input.sld, &regular_tlds, &cache, &resolver).await?;
    dns_failures += summary.dns_failures;
    dns_total += summary.dns_total;
    all_results.extend(summary.results);

    if dns_total > 0 && dns_failures == dns_total && all_results.is_empty() {
        return Err(AppError::network_request("all DNS queries failed"));
    }

    let available_count = all_results.iter().filter(|result| result.available).count();
    let limited_results = visible_results(&all_results, cli.all, cli.limit);
    let stdout = output::render(
        &limited_results,
        OutputOptions {
            json: cli.json,
            show_all: cli.all,
            color: cli.color,
        },
    );

    let mut stderr = Vec::new();
    if dns_failures > 0 {
        stderr.push(format!(
            "note: {dns_failures} of {dns_total} TLDs could not be checked"
        ));
    }

    if available_count == 0 {
        stderr.push(format!(
            "note: no available domains found for '{}'",
            input.normalized
        ));
    }

    if let Some(handle) = update_handle {
        if let Some(notice) = take_if_finished(handle).await {
            stderr.extend(notice.render_lines());
        }
    }

    Ok(RunReport {
        stdout,
        stderr,
        exit_code: if available_count > 0 { 0 } else { 1 },
    })
}

async fn resolve_hacks(
    input: &str,
    trie: &HackTrie,
    cache: &CacheStore,
    resolver: &DnsResolver,
) -> Result<ResolutionSummary, AppError> {
    let matches = trie.find_matches(input);
    let mut summary = ResolutionSummary::default();
    let mut dns_domains = Vec::new();
    let mut dns_positions = Vec::new();

    for hack in matches {
        if hack.sld.len() <= 3 {
            let available = cache.lookup(&hack.sld, &hack.tld)?;
            summary.results.push(DomainResult {
                domain: hack.domain(),
                available,
                kind: ResultKind::Hack,
                method: CheckMethod::Cache,
            });
        } else {
            dns_positions.push(summary.results.len());
            dns_domains.push(hack.domain());
            summary.results.push(DomainResult {
                domain: String::new(),
                available: false,
                kind: ResultKind::Hack,
                method: CheckMethod::Dns,
            });
        }
    }

    if !dns_domains.is_empty() {
        let batch = resolver.check_domains(&dns_domains).await;
        summary.dns_failures += batch.failures;
        summary.dns_total += batch.total;

        for (position, entry) in dns_positions.into_iter().zip(batch.results.into_iter()) {
            if let Ok(available) = entry.available {
                summary.results[position] = DomainResult {
                    domain: entry.domain,
                    available,
                    kind: ResultKind::Hack,
                    method: CheckMethod::Dns,
                };
            }
        }

        summary.results.retain(|result| !result.domain.is_empty());
    }

    Ok(summary)
}

async fn resolve_regular(
    sld: &str,
    tlds: &[String],
    cache: &CacheStore,
    resolver: &DnsResolver,
) -> Result<ResolutionSummary, AppError> {
    if sld.len() <= 3 {
        let results = tlds
            .iter()
            .map(|tld| {
                Ok(DomainResult {
                    domain: format!("{sld}.{tld}"),
                    available: cache.lookup(sld, tld)?,
                    kind: ResultKind::Regular,
                    method: CheckMethod::Cache,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;

        return Ok(ResolutionSummary {
            results,
            dns_failures: 0,
            dns_total: 0,
        });
    }

    let domains = tlds
        .iter()
        .map(|tld| format!("{sld}.{tld}"))
        .collect::<Vec<_>>();
    let batch = resolver.check_domains(&domains).await;

    let results = batch
        .results
        .into_iter()
        .filter_map(|entry| {
            let available = entry.available.ok()?;
            Some(DomainResult {
                domain: entry.domain,
                available,
                kind: ResultKind::Regular,
                method: CheckMethod::Dns,
            })
        })
        .collect::<Vec<_>>();

    Ok(ResolutionSummary {
        results,
        dns_failures: batch.failures,
        dns_total: batch.total,
    })
}
