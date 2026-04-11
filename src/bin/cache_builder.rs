use clap::{Parser, Subcommand};
use domaingrep::cache::{all_short_domains, CacheFile};
use domaingrep::error::AppError;
use domaingrep::http::build_http_client_with_timeouts;
use domaingrep::resolve::{default_resolvers, load_resolvers_file, resolve_domains, ResolveConfig};
use domaingrep::tld::{fetch_filtered_tlds, sort_tlds, split_groups, DEFAULT_TLD_SOURCE_URL};
use std::cmp::min;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCAN_BATCH_SIZE: usize = 25_000;

#[derive(Debug, Parser)]
#[command(name = "cache-builder", about = "Build the domaingrep bitmap cache")]
struct CacheBuilderCli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    FetchTlds {
        #[arg(long, default_value_t = 40)]
        group_size: usize,

        #[arg(long, default_value = DEFAULT_TLD_SOURCE_URL)]
        source_url: String,

        #[arg(long)]
        resolvers: Option<PathBuf>,

        #[arg(long, default_value_t = 100)]
        concurrency: usize,

        #[arg(long, default_value_t = 500)]
        query_timeout_ms: u64,

        #[arg(long, default_value_t = 4)]
        max_attempts: u8,

        #[arg(long, default_value_t = 5)]
        connect_timeout: u64,

        #[arg(long, default_value_t = 10)]
        request_timeout: u64,
    },
    Scan {
        #[arg(long)]
        tlds: String,

        #[arg(long, default_value = "partial-bitmap.bin")]
        output: PathBuf,

        #[arg(long, default_value_t = 10000)]
        concurrency: usize,

        #[arg(long, default_value_t = 500)]
        query_timeout_ms: u64,

        #[arg(long, default_value_t = 4)]
        max_attempts: u8,

        #[arg(long, default_value_t = 4)]
        socket_count: usize,

        #[arg(long)]
        resolvers: Option<PathBuf>,
    },
    Merge {
        #[arg(long)]
        output: PathBuf,

        #[arg(long, default_value = ".")]
        input_dir: PathBuf,
    },
}

#[tokio::main]
async fn main() {
    let cli = CacheBuilderCli::parse();
    let exit_code = match run(cli).await {
        Ok(()) => 0,
        Err(err) => {
            eprint!("{err}");
            err.exit_code()
        }
    };

    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(exit_code);
}

async fn run(cli: CacheBuilderCli) -> Result<(), AppError> {
    match cli.command {
        Commands::FetchTlds {
            group_size,
            source_url,
            resolvers,
            concurrency,
            query_timeout_ms,
            max_attempts,
            connect_timeout,
            request_timeout,
        } => {
            let client = build_http_client_with_timeouts(
                source_url.starts_with("https://"),
                Duration::from_secs(connect_timeout),
                Duration::from_secs(request_timeout),
            )?;
            let resolve_config =
                build_resolve_config(resolvers, concurrency, query_timeout_ms, max_attempts, 1)?;
            let groups = split_groups(
                &fetch_filtered_tlds(&client, &resolve_config, &source_url).await?,
                group_size,
            );
            let output = serde_json::to_string(&groups)
                .map_err(|err| AppError::io("failed to serialize TLD groups", err))?;
            println!("{output}");
            Ok(())
        }
        Commands::Scan {
            tlds,
            output,
            concurrency,
            query_timeout_ms,
            max_attempts,
            socket_count,
            resolvers,
        } => scan_command(
            &tlds,
            output,
            concurrency,
            query_timeout_ms,
            max_attempts,
            socket_count,
            resolvers,
        ),
        Commands::Merge { output, input_dir } => merge_command(output, input_dir),
    }
}

fn scan_command(
    tlds_input: &str,
    output: PathBuf,
    concurrency: usize,
    query_timeout_ms: u64,
    max_attempts: u8,
    socket_count: usize,
    resolvers_path: Option<PathBuf>,
) -> Result<(), AppError> {
    let mut tlds = parse_tlds(tlds_input)?;
    sort_tlds(&mut tlds);

    let domains = all_short_domains();
    let mut cache = CacheFile::empty(tlds.clone(), now_unix_seconds());
    let resolve_config = build_resolve_config(
        resolvers_path,
        concurrency,
        query_timeout_ms,
        max_attempts,
        socket_count,
    )?;

    let domain_indices: Vec<usize> = domains
        .iter()
        .map(|d| {
            domaingrep::cache::domain_to_index(d).expect("all_short_domains are valid") as usize
        })
        .collect();

    let total_queries = tlds.len() * domains.len();
    eprintln!(
        "Generating {total_queries} queries ({} TLDs × {} domains)...",
        tlds.len(),
        domains.len()
    );

    let mut available_count = 0usize;
    for (tld_index, tld) in tlds.iter().enumerate() {
        let mut start = 0usize;
        while start < domains.len() {
            let end = min(start + SCAN_BATCH_SIZE, domains.len());
            let batch_domains = domains[start..end]
                .iter()
                .map(|domain| format!("{domain}.{tld}"))
                .collect::<Vec<_>>();
            let availability = resolve_domains(&resolve_config, &batch_domains)?;

            for (offset, available) in availability.into_iter().enumerate() {
                if available {
                    cache.set_available_raw(tld_index, domain_indices[start + offset], true)?;
                    available_count += 1;
                }
            }

            start = end;
        }
    }

    eprintln!("Found {available_count} available domains out of {total_queries} queries");

    cache.finalize_checksum();

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| AppError::io(format!("failed to create {}", parent.display()), err))?;
    }

    fs::write(&output, cache.to_bytes())
        .map_err(|err| AppError::io(format!("failed to write {}", output.display()), err))
}

fn merge_command(output: PathBuf, input_dir: PathBuf) -> Result<(), AppError> {
    let partial_paths = collect_partial_files(&input_dir, &output)?;
    if partial_paths.is_empty() {
        return Err(AppError::new("no partial bitmap files found")
            .with_help("run 'cache-builder scan' first or pass the correct --input-dir"));
    }

    let mut partials = Vec::new();
    let mut merged_tlds = Vec::new();

    for path in partial_paths {
        let bytes = fs::read(&path)
            .map_err(|err| AppError::io(format!("failed to read {}", path.display()), err))?;
        let partial = CacheFile::from_bytes(&bytes)?;
        merged_tlds.extend(partial.header.tlds.iter().cloned());
        partials.push(partial);
    }

    sort_tlds(&mut merged_tlds);
    merged_tlds.dedup();

    let mut merged = CacheFile::empty(merged_tlds.clone(), now_unix_seconds());
    for (dst_index, tld) in merged_tlds.iter().enumerate() {
        for partial in &partials {
            let Some(src_index) = partial
                .header
                .tlds
                .iter()
                .position(|candidate| candidate == tld)
            else {
                continue;
            };

            merged.copy_tld_bitmap(dst_index, partial.bitmap(), src_index)?;
            break;
        }
    }
    merged.finalize_checksum();

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| AppError::io(format!("failed to create {}", parent.display()), err))?;
    }

    fs::write(&output, merged.to_bytes())
        .map_err(|err| AppError::io(format!("failed to write {}", output.display()), err))
}

fn parse_tlds(input: &str) -> Result<Vec<String>, AppError> {
    if input.trim_start().starts_with('[') {
        return serde_json::from_str::<Vec<String>>(input).map_err(|_| {
            AppError::new("failed to parse --tlds JSON array")
                .with_help("pass a JSON array like '[\"com\",\"io\"]' or a comma-separated list")
        });
    }

    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for item in input
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        if seen.insert(item.to_string()) {
            output.push(item.to_string());
        }
    }

    if output.is_empty() {
        return Err(AppError::new("no TLDs provided to --tlds")
            .with_help("pass a JSON array or comma-separated list of TLDs"));
    }

    Ok(output)
}

fn build_resolve_config(
    resolvers_path: Option<PathBuf>,
    concurrency: usize,
    query_timeout_ms: u64,
    max_attempts: u8,
    socket_count: usize,
) -> Result<ResolveConfig, AppError> {
    let resolvers = match resolvers_path {
        Some(path) => load_resolvers_file(&path)?,
        None => default_resolvers(),
    };

    Ok(ResolveConfig {
        resolvers,
        concurrency,
        query_timeout_ms,
        max_attempts,
        socket_count,
        ..ResolveConfig::builder_default()
    }
    .normalized())
}

fn collect_partial_files(input_dir: &Path, output: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut files = Vec::new();
    walk_partial_files(input_dir, output, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_partial_files(dir: &Path, output: &Path, files: &mut Vec<PathBuf>) -> Result<(), AppError> {
    for entry in fs::read_dir(dir)
        .map_err(|err| AppError::io(format!("failed to read {}", dir.display()), err))?
    {
        let entry = entry.map_err(|err| AppError::io("failed to read directory entry", err))?;
        let path = entry.path();

        if path == output {
            continue;
        }

        if path.is_dir() {
            walk_partial_files(&path, output, files)?;
            continue;
        }

        if path.file_name().and_then(|name| name.to_str()) == Some("partial-bitmap.bin") {
            files.push(path);
        }
    }

    Ok(())
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_secs() as i64
}
