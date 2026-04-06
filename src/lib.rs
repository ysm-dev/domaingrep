pub mod cache;
pub mod cli;
pub mod error;
pub mod hack;
pub mod http;
pub mod input;
pub mod output;
pub mod resolve;
pub mod tld;
pub mod update;

use cache::{CacheConfig, CacheStore};
use cli::Cli;
use error::AppError;
use hack::HackTrie;
use http::build_http_client;
use input::{parse, InputMode};
use output::{visible_results, CheckMethod, DomainResult, OutputOptions, ResultKind};
use resolve::{parse_resolver_list, resolve_domains, ResolveConfig};
use std::env;
use std::net::SocketAddr;
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
    pub resolvers: Vec<SocketAddr>,
    pub resolve_concurrency: usize,
    pub resolve_timeout_ms: u64,
    pub resolve_attempts: u8,
    pub resolve_socket_count: usize,
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
            resolvers: match env::var("DOMAINGREP_RESOLVERS") {
                Ok(value) => parse_resolver_list(&value)?,
                Err(_) => ResolveConfig::default().resolvers,
            },
            resolve_concurrency: parse_env_usize("DOMAINGREP_RESOLVE_CONCURRENCY", 1_000)?,
            resolve_timeout_ms: parse_env_u64("DOMAINGREP_RESOLVE_TIMEOUT_MS", 500)?,
            resolve_attempts: parse_env_u8("DOMAINGREP_RESOLVE_ATTEMPTS", 4)?,
            resolve_socket_count: parse_env_usize("DOMAINGREP_RESOLVE_SOCKET_COUNT", 1)?,
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
        && runtime.update_api_url.starts_with("https://");
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
    let resolve_config = ResolveConfig {
        resolvers: runtime.resolvers.clone(),
        concurrency: runtime.resolve_concurrency,
        query_timeout_ms: runtime.resolve_timeout_ms,
        max_attempts: runtime.resolve_attempts,
        socket_count: runtime.resolve_socket_count,
        ..ResolveConfig::default()
    }
    .normalized();

    let mut all_results = Vec::new();

    if input.mode == InputMode::SldOnly {
        let hack_tlds = tld::filter_tlds(cache.tlds(), None, tld_range);
        let trie = HackTrie::new(hack_tlds.iter().map(String::as_str));
        let summary = resolve_hacks(&input.sld, &trie, &cache, &resolve_config).await?;
        all_results.extend(summary.results);
    }

    let summary = resolve_regular(&input.sld, &regular_tlds, &cache, &resolve_config).await?;
    all_results.extend(summary.results);

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
    resolve_config: &ResolveConfig,
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
        let config = resolve_config.clone();
        let domains = dns_domains.clone();
        let availability = tokio::task::spawn_blocking(move || resolve_domains(&config, &domains))
            .await
            .map_err(|err| AppError::new(format!("DNS worker task failed: {err}")))??;

        for ((position, domain), available) in dns_positions
            .into_iter()
            .zip(dns_domains.into_iter())
            .zip(availability.into_iter())
        {
            summary.results[position] = DomainResult {
                domain,
                available,
                kind: ResultKind::Hack,
                method: CheckMethod::Dns,
            };
        }
    }

    Ok(summary)
}

async fn resolve_regular(
    sld: &str,
    tlds: &[String],
    cache: &CacheStore,
    resolve_config: &ResolveConfig,
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

        return Ok(ResolutionSummary { results });
    }

    let domains = tlds
        .iter()
        .map(|tld| format!("{sld}.{tld}"))
        .collect::<Vec<_>>();
    let config = resolve_config.clone();
    let query_domains = domains.clone();
    let availability =
        tokio::task::spawn_blocking(move || resolve_domains(&config, &query_domains))
            .await
            .map_err(|err| AppError::new(format!("DNS worker task failed: {err}")))??;

    let results = domains
        .into_iter()
        .zip(availability.into_iter())
        .map(|(domain, available)| DomainResult {
            domain,
            available,
            kind: ResultKind::Regular,
            method: CheckMethod::Dns,
        })
        .collect::<Vec<_>>();

    Ok(ResolutionSummary { results })
}

fn parse_env_usize(name: &str, default: usize) -> Result<usize, AppError> {
    match env::var(name) {
        Ok(value) => value.parse::<usize>().map_err(|_| {
            AppError::new(format!("invalid value '{value}' for {name}"))
                .with_help("use a positive integer")
        }),
        Err(_) => Ok(default),
    }
}

fn parse_env_u64(name: &str, default: u64) -> Result<u64, AppError> {
    match env::var(name) {
        Ok(value) => value.parse::<u64>().map_err(|_| {
            AppError::new(format!("invalid value '{value}' for {name}"))
                .with_help("use a positive integer")
        }),
        Err(_) => Ok(default),
    }
}

fn parse_env_u8(name: &str, default: u8) -> Result<u8, AppError> {
    match env::var(name) {
        Ok(value) => value.parse::<u8>().map_err(|_| {
            AppError::new(format!("invalid value '{value}' for {name}"))
                .with_help("use a positive integer between 1 and 255")
        }),
        Err(_) => Ok(default),
    }
}
