use std::error::Error;
use std::fmt::{self, Display, Formatter};

#[derive(Debug, Clone)]
pub struct AppError {
    message: String,
    where_line: Option<String>,
    help: Option<String>,
    exit_code: i32,
}

impl AppError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            where_line: None,
            help: None,
            exit_code: 2,
        }
    }

    pub fn raw(message: impl Into<String>, exit_code: i32) -> Self {
        Self {
            message: message.into(),
            where_line: None,
            help: None,
            exit_code,
        }
    }

    pub fn with_where(mut self, where_line: impl Into<String>) -> Self {
        self.where_line = Some(where_line.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_exit_code(mut self, exit_code: i32) -> Self {
        self.exit_code = exit_code;
        self
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub fn no_domain() -> Self {
        Self::new("no domain provided").with_help("pass a single domain label like 'abc'")
    }

    pub fn limit_must_be_at_least_one() -> Self {
        Self::new("--limit must be at least 1")
            .with_help("use a positive integer such as '--limit 10'")
    }

    pub fn invalid_tld_len(input: &str) -> Self {
        Self::new(format!("invalid --tld-len range '{input}'"))
            .with_help("use '2', '2..5', '..3', or '4..'")
    }

    pub fn cache_dir_unavailable() -> Self {
        Self::new("unable to determine cache directory")
            .with_help("set DOMAINGREP_CACHE_DIR or use a platform with a standard cache directory")
    }

    pub fn cache_download_failed() -> Self {
        Self::new("failed to download domain cache from GitHub Releases")
            .with_help("check your network connection and try again")
    }

    pub fn cache_integrity_failed() -> Self {
        Self::new("cache integrity check failed (SHA-256 mismatch)")
            .with_help("delete the local cache and retry the command")
    }

    pub fn network_request(err: impl Display) -> Self {
        Self::new(format!("network request failed: {err}"))
            .with_help("check your network connection and try again")
    }

    pub fn io(context: impl Display, err: impl Display) -> Self {
        Self::new(format!("{context}: {err}"))
    }
}

impl Display for AppError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "error: {}", self.message)?;
        if let Some(where_line) = &self.where_line {
            writeln!(f, "  --> {where_line}")?;
        }
        if let Some(help) = &self.help {
            writeln!(f, "  = help: {help}")?;
        }
        Ok(())
    }
}

impl Error for AppError {}
