mod common;

use common::{
    cache_fixture, cache_fixture_at, gzip_bytes, now_unix_seconds, sha256_hex, write_cache_meta,
};
use domaingrep::cache::{domain_to_index, CacheConfig, CacheFile, CacheStore};
use domaingrep::http::build_http_client;
use std::time::Duration;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn computes_domain_indices_from_spec_examples() {
    assert_eq!(domain_to_index("abc").unwrap(), 1_370);
    assert_eq!(domain_to_index("a-b").unwrap(), 2_629);
}

#[test]
fn cache_round_trips_and_looks_up_domains() {
    let cache = cache_fixture(&["ai", "com", "io"], &[("abc", "com"), ("a-b", "io")]);
    let bytes = cache.to_bytes();
    let parsed = CacheFile::from_bytes(&bytes).unwrap();

    assert!(parsed.lookup_by_index(1, "abc").unwrap());
    assert!(parsed.lookup_by_index(2, "a-b").unwrap());
    assert!(!parsed.lookup_by_index(0, "abc").unwrap());
}

#[tokio::test]
async fn downloads_and_verifies_cache_assets() {
    let server = MockServer::start().await;
    let temp = tempdir().unwrap();
    let cache = cache_fixture(&["io", "com"], &[("abc", "io")]);
    let compressed = gzip_bytes(&cache.to_bytes());
    let checksum = format!("{}  cache.bin.gz\n", sha256_hex(&compressed));

    Mock::given(method("GET"))
        .and(path("/cache.bin.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(compressed))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cache.sha256"))
        .respond_with(ResponseTemplate::new(200).set_body_string(checksum))
        .mount(&server)
        .await;

    let store = CacheStore::load_or_update(
        build_http_client(false).unwrap(),
        CacheConfig {
            cache_dir: temp.path().join("cache"),
            asset_url: format!("{}/cache.bin.gz", server.uri()),
            checksum_url: format!("{}/cache.sha256", server.uri()),
        },
    )
    .await
    .unwrap();

    assert!(store.lookup("abc", "io").unwrap());
    assert!(!store.lookup("abc", "com").unwrap());
}

#[tokio::test]
async fn corrupt_local_cache_is_replaced_from_remote() {
    let server = MockServer::start().await;
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("cache.bin"), b"corrupt").unwrap();

    let cache = cache_fixture(&["sh"], &[("abc", "sh")]);
    let compressed = gzip_bytes(&cache.to_bytes());
    let checksum = format!("{}  cache.bin.gz\n", sha256_hex(&compressed));

    Mock::given(method("GET"))
        .and(path("/cache.bin.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(compressed))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cache.sha256"))
        .respond_with(ResponseTemplate::new(200).set_body_string(checksum))
        .mount(&server)
        .await;

    let store = CacheStore::load_or_update(
        build_http_client(false).unwrap(),
        CacheConfig {
            cache_dir,
            asset_url: format!("{}/cache.bin.gz", server.uri()),
            checksum_url: format!("{}/cache.sha256", server.uri()),
        },
    )
    .await
    .unwrap();

    assert!(store.lookup("abc", "sh").unwrap());
}

#[tokio::test]
async fn stale_cache_is_served_while_refresh_happens_in_background() {
    let server = MockServer::start().await;
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let stale_timestamp = now_unix_seconds() - (25 * 60 * 60);
    let stale_cache = cache_fixture_at(&["io"], &[], stale_timestamp);
    std::fs::write(cache_dir.join("cache.bin"), stale_cache.to_bytes()).unwrap();
    write_cache_meta(
        &cache_dir,
        stale_timestamp,
        &format!("{}/cache.bin.gz", server.uri()),
        "stale",
    );

    let fresh_cache = cache_fixture(&["io"], &[("abc", "io")]);
    let compressed = gzip_bytes(&fresh_cache.to_bytes());
    let checksum = format!("{}  cache.bin.gz\n", sha256_hex(&compressed));

    Mock::given(method("GET"))
        .and(path("/cache.bin.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(compressed))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cache.sha256"))
        .respond_with(ResponseTemplate::new(200).set_body_string(checksum))
        .mount(&server)
        .await;

    let config = CacheConfig {
        cache_dir: cache_dir.clone(),
        asset_url: format!("{}/cache.bin.gz", server.uri()),
        checksum_url: format!("{}/cache.sha256", server.uri()),
    };

    let stale_store = CacheStore::load_or_update(build_http_client(false).unwrap(), config.clone())
        .await
        .unwrap();
    assert!(!stale_store.lookup("abc", "io").unwrap());

    let mut refreshed = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let store = CacheStore::load_or_update(build_http_client(false).unwrap(), config.clone())
            .await
            .unwrap();
        if store.lookup("abc", "io").unwrap() {
            refreshed = true;
            break;
        }
    }

    assert!(refreshed, "stale cache should refresh in the background");
}
