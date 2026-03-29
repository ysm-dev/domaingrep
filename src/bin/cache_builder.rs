use clap::{Parser, Subcommand};
use domaingrep::cache::{all_short_domains, CacheFile, DOMAINS_PER_TLD};
use domaingrep::dns::{
    build_http_client, DnsResolver, DEFAULT_FALLBACK_DOH_URL, DEFAULT_PRIMARY_DOH_URL,
};
use domaingrep::error::AppError;
use domaingrep::tld::{fetch_filtered_tlds, sort_tlds, split_groups, DEFAULT_TLD_SOURCE_URL};
use futures::{stream, StreamExt};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

        #[arg(long, default_value = DEFAULT_PRIMARY_DOH_URL)]
        primary_url: String,

        #[arg(long, default_value = DEFAULT_FALLBACK_DOH_URL)]
        fallback_url: String,
    },
    Scan {
        #[arg(long)]
        tlds: String,

        #[arg(long, default_value = "partial-bitmap.bin")]
        output: PathBuf,

        #[arg(long, default_value_t = 100)]
        concurrency: usize,

        #[arg(long, default_value = DEFAULT_PRIMARY_DOH_URL)]
        primary_url: String,

        #[arg(long, default_value = DEFAULT_FALLBACK_DOH_URL)]
        fallback_url: String,
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

    std::process::exit(exit_code);
}

async fn run(cli: CacheBuilderCli) -> Result<(), AppError> {
    match cli.command {
        Commands::FetchTlds {
            group_size,
            source_url,
            primary_url,
            fallback_url,
        } => {
            let client = build_http_client(
                primary_url.starts_with("https://") && fallback_url.starts_with("https://"),
            )?;
            let resolver = DnsResolver::new(client.clone(), primary_url, fallback_url);
            let groups = split_groups(
                &fetch_filtered_tlds(&client, &resolver, &source_url).await?,
                group_size,
            );
            let output = serde_json::to_string_pretty(&groups)
                .map_err(|err| AppError::io("failed to serialize TLD groups", err))?;
            println!("{output}");
            Ok(())
        }
        Commands::Scan {
            tlds,
            output,
            concurrency,
            primary_url,
            fallback_url,
        } => scan_command(&tlds, output, concurrency, &primary_url, &fallback_url).await,
        Commands::Merge { output, input_dir } => merge_command(output, input_dir),
    }
}

async fn scan_command(
    tlds_input: &str,
    output: PathBuf,
    concurrency: usize,
    primary_url: &str,
    fallback_url: &str,
) -> Result<(), AppError> {
    let mut tlds = parse_tlds(tlds_input)?;
    sort_tlds(&mut tlds);

    let client = build_http_client(
        primary_url.starts_with("https://") && fallback_url.starts_with("https://"),
    )?;
    let resolver = DnsResolver::with_concurrency(
        client,
        primary_url.to_string(),
        fallback_url.to_string(),
        concurrency,
    );
    let domains = all_short_domains();
    let mut cache = CacheFile::empty(tlds.clone(), now_unix_seconds());

    for (tld_index, tld) in tlds.iter().enumerate() {
        let requests = stream::iter(domains.iter().map(|domain| {
            let resolver = resolver.clone();
            let tld = tld.clone();
            let domain = domain.clone();
            async move {
                let full_domain = format!("{domain}.{tld}");
                let available = scan_domain(&resolver, &full_domain).await;
                (domain, available)
            }
        }))
        .buffer_unordered(concurrency.max(1));

        tokio::pin!(requests);
        while let Some((domain, available)) = requests.next().await {
            if available {
                cache.set_available_by_index(tld_index, &domain, true)?;
            }
        }
    }

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| AppError::io(format!("failed to create {}", parent.display()), err))?;
    }

    fs::write(&output, cache.to_bytes())
        .map_err(|err| AppError::io(format!("failed to write {}", output.display()), err))
}

async fn scan_domain(resolver: &DnsResolver, domain: &str) -> bool {
    for _ in 0..3 {
        match resolver.check_availability(domain).await {
            Ok(available) => return available,
            Err(_) => continue,
        }
    }
    false
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

            for domain_index in 0..DOMAINS_PER_TLD {
                if partial.is_available_raw(src_index, domain_index) {
                    merged.set_available_raw(dst_index, domain_index, true)?;
                }
            }

            break;
        }
    }

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
