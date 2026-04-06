use crate::error::AppError;
use reqwest::Client;
use std::time::Duration;

pub fn build_http_client(force_http2: bool) -> Result<Client, AppError> {
    build_http_client_with_timeouts(force_http2, Duration::from_secs(5), Duration::from_secs(10))
}

pub fn build_http_client_with_timeouts(
    force_http2: bool,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<Client, AppError> {
    let builder = Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .user_agent(format!("domaingrep/{}", env!("CARGO_PKG_VERSION")))
        .pool_max_idle_per_host(50);

    let builder = if force_http2 {
        builder.http2_prior_knowledge()
    } else {
        builder
    };

    builder.build().map_err(AppError::network_request)
}
