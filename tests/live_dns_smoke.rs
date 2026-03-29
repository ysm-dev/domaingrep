use domaingrep::dns::{build_http_client, DnsResolver};

#[tokio::test]
#[ignore]
async fn known_registered_domain_is_unavailable() {
    let resolver = DnsResolver::new(
        build_http_client(true).unwrap(),
        "https://cloudflare-dns.com/dns-query",
        "https://dns.google/resolve",
    );

    assert!(!resolver.check_availability("google.com").await.unwrap());
}

#[tokio::test]
#[ignore]
async fn unlikely_domain_is_available() {
    let resolver = DnsResolver::new(
        build_http_client(true).unwrap(),
        "https://cloudflare-dns.com/dns-query",
        "https://dns.google/resolve",
    );

    assert!(resolver
        .check_availability("xyzzy-test-domain-12345.com")
        .await
        .unwrap());
}
