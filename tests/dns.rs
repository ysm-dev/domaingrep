use domaingrep::dns::{build_http_client, DnsResolver};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn nxdomain_is_available() {
    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/dns-query"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"Status\":3}", "application/json"))
        .mount(&primary)
        .await;

    let resolver = DnsResolver::new(
        build_http_client(false).unwrap(),
        format!("{}/dns-query", primary.uri()),
        format!("{}/resolve", fallback.uri()),
    );

    assert!(resolver.check_availability("example.com").await.unwrap());
}

#[tokio::test]
async fn servfail_uses_fallback_provider() {
    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/dns-query"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"Status\":2}", "application/json"))
        .mount(&primary)
        .await;
    Mock::given(method("GET"))
        .and(path("/resolve"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"Status\":0}", "application/json"))
        .mount(&fallback)
        .await;

    let resolver = DnsResolver::new(
        build_http_client(false).unwrap(),
        format!("{}/dns-query", primary.uri()),
        format!("{}/resolve", fallback.uri()),
    );

    assert!(!resolver.check_availability("example.com").await.unwrap());
}

#[tokio::test]
async fn http_429_uses_fallback_provider() {
    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/dns-query"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&primary)
        .await;
    Mock::given(method("GET"))
        .and(path("/resolve"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"Status\":3}", "application/json"))
        .mount(&fallback)
        .await;

    let resolver = DnsResolver::new(
        build_http_client(false).unwrap(),
        format!("{}/dns-query", primary.uri()),
        format!("{}/resolve", fallback.uri()),
    );

    assert!(resolver.check_availability("example.com").await.unwrap());
}

#[tokio::test]
async fn batch_counts_failures_when_both_providers_fail() {
    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/dns-query"))
        .and(query_param("name", "bad.example"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&primary)
        .await;
    Mock::given(method("GET"))
        .and(path("/resolve"))
        .and(query_param("name", "bad.example"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&fallback)
        .await;

    let resolver = DnsResolver::new(
        build_http_client(false).unwrap(),
        format!("{}/dns-query", primary.uri()),
        format!("{}/resolve", fallback.uri()),
    );
    let batch = resolver.check_domains(&["bad.example".to_string()]).await;

    assert_eq!(batch.failures, 1);
    assert!(batch.results[0].available.is_err());
}

#[tokio::test]
async fn fallback_terminal_dns_status_is_treated_as_unavailable() {
    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/dns-query"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"Status\":2}", "application/json"))
        .mount(&primary)
        .await;
    Mock::given(method("GET"))
        .and(path("/resolve"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"Status\":5}", "application/json"))
        .mount(&fallback)
        .await;

    let resolver = DnsResolver::new(
        build_http_client(false).unwrap(),
        format!("{}/dns-query", primary.uri()),
        format!("{}/resolve", fallback.uri()),
    );

    assert!(!resolver.check_availability("example.com").await.unwrap());
}
