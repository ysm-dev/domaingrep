use crate::error::AppError;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

pub const DEFAULT_UPDATE_API_URL: &str =
    "https://api.github.com/repos/ysm-dev/domaingrep/releases/latest";
const CHECK_INTERVAL_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub cache_dir: PathBuf,
    pub api_url: String,
    pub current_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateNotice {
    pub latest_version: String,
    pub current_version: String,
}

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
}

impl UpdateNotice {
    pub fn render_lines(&self) -> Vec<String> {
        vec![
            format!(
                "note: domaingrep v{} is available (current: v{})",
                self.latest_version, self.current_version
            ),
            "  -> brew upgrade domaingrep".to_string(),
            "  -> cargo install domaingrep".to_string(),
            "  -> curl -fsSL https://domaingrep.dev/install.sh | sh".to_string(),
        ]
    }
}

pub fn maybe_start(
    client: Client,
    config: UpdateConfig,
) -> Result<Option<JoinHandle<Option<UpdateNotice>>>, AppError> {
    if !should_check(&config.cache_dir.join("last_update_check"))? {
        return Ok(None);
    }

    Ok(Some(tokio::spawn(async move {
        perform_check(client, config).await.ok().flatten()
    })))
}

pub async fn take_if_finished(handle: JoinHandle<Option<UpdateNotice>>) -> Option<UpdateNotice> {
    if handle.is_finished() {
        handle.await.ok().flatten()
    } else {
        None
    }
}

async fn perform_check(
    client: Client,
    config: UpdateConfig,
) -> Result<Option<UpdateNotice>, AppError> {
    let response = client
        .get(&config.api_url)
        .send()
        .await
        .map_err(AppError::network_request)?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let latest = response
        .json::<LatestRelease>()
        .await
        .map_err(AppError::network_request)?;

    write_timestamp(
        &config.cache_dir.join("last_update_check"),
        now_unix_seconds(),
    )?;

    let current = parse_version(&config.current_version)?;
    let latest_version = parse_version(latest.tag_name.trim())?;

    if latest_version > current {
        return Ok(Some(UpdateNotice {
            latest_version: latest_version.to_string(),
            current_version: current.to_string(),
        }));
    }

    Ok(None)
}

fn parse_version(input: &str) -> Result<Version, AppError> {
    Version::parse(input.trim_start_matches('v')).map_err(|_| {
        AppError::new(format!("invalid release version '{input}'"))
            .with_help("expected a semantic version like v0.3.0")
    })
}

fn should_check(path: &Path) -> Result<bool, AppError> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Ok(true);
    };

    let Ok(timestamp) = contents.trim().parse::<i64>() else {
        return Ok(true);
    };

    Ok(now_unix_seconds() - timestamp >= CHECK_INTERVAL_SECONDS)
}

fn write_timestamp(path: &Path, timestamp: i64) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| AppError::io("failed to create cache directory", err))?;
    }

    fs::write(path, timestamp.to_string())
        .map_err(|err| AppError::io(format!("failed to write {}", path.display()), err))
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_secs() as i64
}
