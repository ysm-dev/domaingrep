mod engine;
mod slab;
mod socket;
mod wheel;
mod wire;

use crate::error::AppError;
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::str::FromStr;

pub const QTYPE_NS: u16 = 2;
pub const RCODE_NOERROR: u8 = 0;
pub const RCODE_NXDOMAIN: u8 = 3;

const DEFAULT_RESOLVER_LIST: &str = "\
1.1.1.1
1.0.0.1
8.8.8.8
8.8.4.4
9.9.9.9
149.112.112.112
208.67.222.222
208.67.220.220
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveResponse {
    pub rcode: u8,
    pub answer_count: u16,
}

#[derive(Debug, Clone)]
pub struct ResolveConfig {
    pub resolvers: Vec<SocketAddr>,
    pub concurrency: usize,
    pub query_timeout_ms: u64,
    pub max_attempts: u8,
    pub socket_count: usize,
    pub send_batch_size: usize,
    pub recv_batch_size: usize,
    pub recv_buf_size: usize,
    pub send_buf_size: usize,
}

impl Default for ResolveConfig {
    fn default() -> Self {
        Self {
            resolvers: default_resolvers(),
            concurrency: 1_000,
            query_timeout_ms: 500,
            max_attempts: 4,
            socket_count: 1,
            send_batch_size: 64,
            recv_batch_size: 64,
            recv_buf_size: 4 << 20,
            send_buf_size: 4 << 20,
        }
    }
}

impl ResolveConfig {
    pub fn builder_default() -> Self {
        Self {
            concurrency: 10_000,
            max_attempts: 4,
            socket_count: if cfg!(target_os = "linux") { 4 } else { 1 },
            ..Self::default()
        }
    }

    pub fn normalized(mut self) -> Self {
        self.concurrency = self.concurrency.clamp(1, 60_000);
        self.query_timeout_ms = self.query_timeout_ms.max(1);
        self.max_attempts = self.max_attempts.max(1);
        self.socket_count = self.socket_count.max(1);
        self.send_batch_size = self.send_batch_size.max(1);
        self.recv_batch_size = self.recv_batch_size.max(1);
        self.recv_buf_size = self.recv_buf_size.max(64 * 1024);
        self.send_buf_size = self.send_buf_size.max(64 * 1024);
        self
    }
}

pub fn default_resolvers() -> Vec<SocketAddr> {
    parse_resolver_list(DEFAULT_RESOLVER_LIST)
        .expect("embedded resolver list should always be valid")
}

pub fn parse_resolver_list(input: &str) -> Result<Vec<SocketAddr>, AppError> {
    let resolvers = input
        .split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
        .map(str::trim)
        .filter(|entry| !entry.is_empty() && !entry.starts_with('#'))
        .map(parse_resolver)
        .collect::<Result<Vec<_>, _>>()?;

    if resolvers.is_empty() {
        return Err(AppError::new("no DNS resolvers configured")
            .with_help("set DOMAINGREP_RESOLVERS or provide at least one resolver"));
    }

    Ok(resolvers)
}

pub fn load_resolvers_file(path: &Path) -> Result<Vec<SocketAddr>, AppError> {
    let contents = fs::read_to_string(path)
        .map_err(|err| AppError::io(format!("failed to read {}", path.display()), err))?;
    parse_resolver_list(&contents)
}

pub fn is_available(response: Option<ResolveResponse>) -> bool {
    response.is_some_and(|response| response.rcode == RCODE_NXDOMAIN)
}

pub fn resolve_domains(config: &ResolveConfig, domains: &[String]) -> Result<Vec<bool>, AppError> {
    resolve_domains_raw(config, domains)
        .map(|responses| responses.into_iter().map(is_available).collect())
}

pub fn resolve_domains_raw(
    config: &ResolveConfig,
    domains: &[String],
) -> Result<Vec<Option<ResolveResponse>>, AppError> {
    engine::resolve_raw_domains(&config.clone().normalized(), domains)
}

pub(crate) fn is_definitive(rcode: u8) -> bool {
    matches!(rcode, RCODE_NOERROR | RCODE_NXDOMAIN)
}

fn parse_resolver(token: &str) -> Result<SocketAddr, AppError> {
    if let Ok(addr) = SocketAddr::from_str(token) {
        return Ok(addr);
    }

    if let Ok(ip) = IpAddr::from_str(token) {
        return Ok(SocketAddr::new(ip, 53));
    }

    Err(AppError::new(format!("invalid DNS resolver '{token}'"))
        .with_help("use an IP address or socket address like '1.1.1.1' or '1.1.1.1:53'"))
}

#[cfg(test)]
mod tests {
    use super::parse_resolver_list;

    #[test]
    fn parses_resolvers_with_default_ports() {
        let resolvers = parse_resolver_list("1.1.1.1 8.8.8.8:54").unwrap();
        assert_eq!(resolvers[0].to_string(), "1.1.1.1:53");
        assert_eq!(resolvers[1].to_string(), "8.8.8.8:54");
    }
}
