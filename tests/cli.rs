mod common;

use assert_cmd::prelude::*;
use common::write_local_cache;
use predicates::prelude::*;
use std::process::Command;
use tempfile::tempdir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn short_cache_queries_emit_available_results_only() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(
        &cache_dir,
        &["io", "co", "com", "dev"],
        &[("abc", "io"), ("abc", "dev")],
    );

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("abc")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1");

    command
        .assert()
        .success()
        .stdout("abc.io\nabc.dev\n")
        .stderr("");
}

#[tokio::test]
async fn domain_hacks_appear_before_regular_results() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["sh", "io"], &[("ab", "sh")]);

    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/dns-query"))
        .and(query_param("name", "absh.io"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"Status\":3}", "application/json"))
        .mount(&primary)
        .await;
    Mock::given(method("GET"))
        .and(path("/dns-query"))
        .and(query_param("name", "absh.sh"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"Status\":0}", "application/json"))
        .mount(&primary)
        .await;

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("absh")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1")
        .env(
            "DOMAINGREP_DOH_PRIMARY_URL",
            format!("{}/dns-query", primary.uri()),
        )
        .env(
            "DOMAINGREP_DOH_FALLBACK_URL",
            format!("{}/resolve", fallback.uri()),
        );

    command
        .assert()
        .success()
        .stdout("ab.sh\nabsh.io\n")
        .stderr("");
}

#[test]
fn prefix_mode_disables_hack_detection() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(
        &cache_dir,
        &["sh", "shop", "show", "io"],
        &[("abc", "sh"), ("abc", "shop"), ("abc", "show")],
    );

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("abc.sh")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1");

    command
        .assert()
        .success()
        .stdout("abc.sh\nabc.shop\nabc.show\n")
        .stderr("");
}

#[tokio::test]
async fn dns_mode_reports_partial_failures() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["io", "dev"], &[]);

    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/dns-query"))
        .and(query_param("name", "hello.io"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"Status\":3}", "application/json"))
        .mount(&primary)
        .await;
    Mock::given(method("GET"))
        .and(path("/dns-query"))
        .and(query_param("name", "hello.dev"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&primary)
        .await;
    Mock::given(method("GET"))
        .and(path("/resolve"))
        .and(query_param("name", "hello.dev"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&fallback)
        .await;

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("hello")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1")
        .env(
            "DOMAINGREP_DOH_PRIMARY_URL",
            format!("{}/dns-query", primary.uri()),
        )
        .env(
            "DOMAINGREP_DOH_FALLBACK_URL",
            format!("{}/resolve", fallback.uri()),
        );

    command
        .assert()
        .success()
        .stdout("hello.io\n")
        .stderr(predicate::str::contains(
            "note: 1 of 2 TLDs could not be checked",
        ));
}

#[test]
fn no_available_domains_exit_with_code_one() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["io", "com"], &[]);

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("abc")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1");

    command
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains(
            "note: no available domains found for 'abc'",
        ));
}

#[tokio::test]
async fn complete_dns_failure_is_an_error() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["io", "dev"], &[]);

    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    for name in ["hello.io", "hello.dev"] {
        Mock::given(method("GET"))
            .and(path("/dns-query"))
            .and(query_param("name", name))
            .respond_with(ResponseTemplate::new(500))
            .mount(&primary)
            .await;
        Mock::given(method("GET"))
            .and(path("/resolve"))
            .and(query_param("name", name))
            .respond_with(ResponseTemplate::new(500))
            .mount(&fallback)
            .await;
    }

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("hello")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1")
        .env(
            "DOMAINGREP_DOH_PRIMARY_URL",
            format!("{}/dns-query", primary.uri()),
        )
        .env(
            "DOMAINGREP_DOH_FALLBACK_URL",
            format!("{}/resolve", fallback.uri()),
        );

    command
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains(
            "error: network request failed: all DNS queries failed",
        ));
}
