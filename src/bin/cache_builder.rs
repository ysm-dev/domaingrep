use clap::{Parser, Subcommand};
use domaingrep::cache::{all_short_domains, CacheFile};
use domaingrep::dns::{
    build_http_client_with_timeouts, DnsResolver, DEFAULT_FALLBACK_DOH_URL, DEFAULT_PRIMARY_DOH_URL,
};
use domaingrep::error::AppError;
use domaingrep::tld::{fetch_filtered_tlds, sort_tlds, split_groups, DEFAULT_TLD_SOURCE_URL};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_RESOLVERS: &str = "\
8.8.8.8
8.8.4.4
1.1.1.1
1.0.0.1
9.9.9.9
149.112.112.112
208.67.222.222
208.67.220.220
4.2.2.1
4.2.2.2
64.6.64.6
64.6.65.6
77.88.8.8
77.88.8.1
94.140.14.14
94.140.15.15
";

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

        /// massdns concurrent lookups (hashmap size)
        #[arg(long, default_value_t = 10000)]
        hashmap_size: usize,

        /// Path to a custom resolvers file (one IP per line)
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

    std::process::exit(exit_code);
}

async fn run(cli: CacheBuilderCli) -> Result<(), AppError> {
    match cli.command {
        Commands::FetchTlds {
            group_size,
            source_url,
            primary_url,
            fallback_url,
            connect_timeout,
            request_timeout,
        } => {
            let client = build_http_client_with_timeouts(
                primary_url.starts_with("https://") && fallback_url.starts_with("https://"),
                Duration::from_secs(connect_timeout),
                Duration::from_secs(request_timeout),
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
            hashmap_size,
            resolvers,
        } => scan_command(&tlds, output, hashmap_size, resolvers),
        Commands::Merge { output, input_dir } => merge_command(output, input_dir),
    }
}

fn scan_command(
    tlds_input: &str,
    output: PathBuf,
    hashmap_size: usize,
    resolvers_path: Option<PathBuf>,
) -> Result<(), AppError> {
    let mut tlds = parse_tlds(tlds_input)?;
    sort_tlds(&mut tlds);

    let domains = all_short_domains();
    let mut cache = CacheFile::empty(tlds.clone(), now_unix_seconds());

    let domain_indices: Vec<u32> = domains
        .iter()
        .map(|d| domaingrep::cache::domain_to_index(d).expect("all_short_domains are valid"))
        .collect();

    let domain_pos_map: HashMap<&str, usize> = domains
        .iter()
        .enumerate()
        .map(|(i, d)| (d.as_str(), i))
        .collect();

    let tld_pos_map: HashMap<&str, usize> = tlds
        .iter()
        .enumerate()
        .map(|(i, t)| (t.as_str(), i))
        .collect();

    // --- Prepare temp files for massdns ---
    let temp_dir = std::env::temp_dir().join(format!("cache-builder-{}", std::process::id()));
    fs::create_dir_all(&temp_dir)
        .map_err(|err| AppError::io("failed to create temp directory", err))?;

    let domains_file = temp_dir.join("domains.txt");
    let resolvers_file = temp_dir.join("resolvers.txt");
    let results_file = temp_dir.join("results.jsonl");

    let total_queries = tlds.len() * domains.len();
    eprintln!(
        "Generating {total_queries} queries ({} TLDs × {} domains)...",
        tlds.len(),
        domains.len()
    );

    {
        let mut f = fs::File::create(&domains_file)
            .map_err(|err| AppError::io("failed to create domains file", err))?;
        for tld in &tlds {
            for domain in &domains {
                writeln!(f, "{domain}.{tld}")
                    .map_err(|err| AppError::io("failed to write domain", err))?;
            }
        }
    }

    let resolvers_content = match &resolvers_path {
        Some(path) => fs::read_to_string(path)
            .map_err(|err| AppError::io(format!("failed to read {}", path.display()), err))?,
        None => DEFAULT_RESOLVERS.to_string(),
    };
    fs::write(&resolvers_file, &resolvers_content)
        .map_err(|err| AppError::io("failed to write resolvers file", err))?;

    // --- Run massdns ---
    eprintln!("Running massdns (hashmap-size={hashmap_size})...");

    let massdns_output = Command::new("massdns")
        .args([
            "-r",
            resolvers_file.to_str().unwrap(),
            "-t",
            "A",
            "-o",
            "J",
            "-s",
            &hashmap_size.to_string(),
            "--retry",
            "2",
            "--lifetime",
            "5",
            "-w",
            results_file.to_str().unwrap(),
        ])
        .arg(&domains_file)
        .output()
        .map_err(|err| AppError::io("failed to execute massdns", err))?;

    if !massdns_output.status.success() {
        let stderr = String::from_utf8_lossy(&massdns_output.stderr);
        let stdout = String::from_utf8_lossy(&massdns_output.stdout);
        eprintln!("massdns stderr: {stderr}");
        eprintln!("massdns stdout: {stdout}");
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(AppError::new(format!(
            "massdns exited with status {}",
            massdns_output.status
        ))
        .with_help("check massdns output above for details"));
    }

    // --- Parse results ---
    eprintln!("Parsing massdns results...");

    let reader = BufReader::new(
        fs::File::open(&results_file)
            .map_err(|err| AppError::io("failed to open massdns results", err))?,
    );

    let mut available_count = 0usize;
    for line in reader.lines() {
        let line = line.map_err(|err| AppError::io("failed to read result line", err))?;
        if line.is_empty() {
            continue;
        }

        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let status = match v.get("status").and_then(|s| s.as_str()) {
            Some(s) => s,
            None => continue,
        };

        if status != "NXDOMAIN" {
            continue;
        }

        let name = match v.get("name").and_then(|n| n.as_str()) {
            Some(n) => n.trim_end_matches('.'),
            None => continue,
        };

        // Split into domain and tld (all TLDs in the system are single-label)
        let Some(dot_pos) = name.find('.') else {
            continue;
        };
        let domain_part = &name[..dot_pos];
        let tld_part = &name[dot_pos + 1..];

        if let (Some(&tld_idx), Some(&domain_pos)) =
            (tld_pos_map.get(tld_part), domain_pos_map.get(domain_part))
        {
            cache.set_available_raw(tld_idx, domain_indices[domain_pos] as usize, true)?;
            available_count += 1;
        }
    }

    let _ = fs::remove_dir_all(&temp_dir);

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
