mod common;

use assert_cmd::prelude::*;
use common::{write_local_cache, MockDnsAction, MockDnsServer};
use predicates::prelude::*;
use std::process::Command;
use tempfile::tempdir;

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

#[test]
fn domain_hacks_appear_before_regular_results() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["sh", "io"], &[("ab", "sh")]);

    let server = MockDnsServer::start([
        ("absh.io", vec![MockDnsAction::reply(3)]),
        ("absh.sh", vec![MockDnsAction::reply(0)]),
    ]);

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("absh")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1")
        .env("DOMAINGREP_RESOLVERS", server.addr().to_string())
        .env("DOMAINGREP_RESOLVE_TIMEOUT_MS", "10")
        .env("DOMAINGREP_RESOLVE_ATTEMPTS", "2");

    command
        .assert()
        .success()
        .stdout("ab.sh\nabsh.io\n")
        .stderr("");
}

#[test]
fn available_pinned_regular_results_appear_first() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(
        &cache_dir,
        &["io", "me", "com", "dev", "art"],
        &[
            ("abc", "io"),
            ("abc", "me"),
            ("abc", "com"),
            ("abc", "dev"),
            ("abc", "art"),
        ],
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
        .stdout("abc.com\nabc.io\nabc.me\nabc.dev\nabc.art\n")
        .stderr("");
}

#[test]
fn all_output_only_promotes_available_pinned_results() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["io", "dev"], &[("abc", "dev")]);

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("abc")
        .arg("--all")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1");

    command
        .assert()
        .success()
        .stdout("  abc.dev\nx abc.io\n")
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

#[test]
fn dns_failures_are_collapsed_to_unavailable_without_note() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["io", "dev"], &[]);

    let server = MockDnsServer::start([
        ("hello.io", vec![MockDnsAction::reply(3)]),
        ("hello.dev", vec![MockDnsAction::drop()]),
    ]);

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("hello")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1")
        .env("DOMAINGREP_RESOLVERS", server.addr().to_string())
        .env("DOMAINGREP_RESOLVE_TIMEOUT_MS", "10")
        .env("DOMAINGREP_RESOLVE_ATTEMPTS", "2");

    command.assert().success().stdout("hello.io\n").stderr("");
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

#[test]
fn complete_dns_failures_become_no_available_results() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["io", "dev"], &[]);

    let server = MockDnsServer::start([
        ("hello.io", vec![MockDnsAction::drop()]),
        ("hello.dev", vec![MockDnsAction::drop()]),
    ]);

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("hello")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1")
        .env("DOMAINGREP_RESOLVERS", server.addr().to_string())
        .env("DOMAINGREP_RESOLVE_TIMEOUT_MS", "10")
        .env("DOMAINGREP_RESOLVE_ATTEMPTS", "2");

    command.assert().code(1).stdout("").stderr(
        predicate::str::contains("note: no available domains found for 'hello'")
            .and(predicate::str::contains("could not be checked").not())
            .and(predicate::str::contains("error:").not()),
    );
}

#[test]
fn trailing_dot_is_ignored_before_dot_count_validation() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(
        &cache_dir,
        &["co", "com", "io"],
        &[("abc", "co"), ("abc", "com")],
    );

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("abc.co.")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1");

    command
        .assert()
        .success()
        .stdout("abc.co\nabc.com\n")
        .stderr("");
}

#[test]
fn help_shows_required_domain() {
    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command.arg("--help");

    command
        .assert()
        .success()
        .stdout(predicate::str::contains("domaingrep [OPTIONS] <DOMAIN>"));
}

#[test]
fn explicit_limit_emits_truncation_note() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["io", "dev"], &[("abc", "io"), ("abc", "dev")]);

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("--limit")
        .arg("1")
        .arg("abc")
        .arg("--color")
        .arg("never")
        .env("DOMAINGREP_CACHE_DIR", &cache_dir)
        .env("DOMAINGREP_DISABLE_UPDATE", "1");

    command
        .assert()
        .success()
        .stdout("abc.io\n")
        .stderr(predicate::str::contains(
            "note: 1 more domains not shown (showing 1 of 2; use --limit 0 to show all)",
        ));
}

#[test]
fn limit_zero_shows_all_results() {
    let temp = tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    write_local_cache(&cache_dir, &["io", "dev"], &[("abc", "io"), ("abc", "dev")]);

    let mut command = Command::cargo_bin("domaingrep").unwrap();
    command
        .arg("--limit")
        .arg("0")
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
