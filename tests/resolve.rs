mod common;

use common::{MockDnsAction, MockDnsServer};
use domaingrep::resolve::{resolve_domains, resolve_domains_raw, ResolveConfig};

fn test_config(server: &MockDnsServer) -> ResolveConfig {
    ResolveConfig {
        resolvers: vec![server.addr()],
        concurrency: 32,
        query_timeout_ms: 10,
        max_attempts: 3,
        socket_count: 1,
        send_batch_size: 16,
        recv_batch_size: 16,
        recv_buf_size: 1 << 20,
        send_buf_size: 1 << 20,
    }
}

#[test]
fn nxdomain_is_available_and_noerror_is_unavailable() {
    let server = MockDnsServer::start([
        ("available.example", vec![MockDnsAction::reply(3)]),
        ("taken.example", vec![MockDnsAction::reply(0)]),
    ]);

    let results = resolve_domains(
        &test_config(&server),
        &["available.example".to_string(), "taken.example".to_string()],
    )
    .unwrap();

    assert_eq!(results, vec![true, false]);
}

#[test]
fn raw_results_include_answer_counts() {
    let server = MockDnsServer::start([
        ("nic.example", vec![MockDnsAction::reply_with_answers(0, 2)]),
        (
            "probe.example",
            vec![MockDnsAction::reply_with_answers(3, 0)],
        ),
    ]);

    let results = resolve_domains_raw(
        &test_config(&server),
        &["nic.example".to_string(), "probe.example".to_string()],
    )
    .unwrap();

    assert_eq!(results[0].unwrap().rcode, 0);
    assert_eq!(results[0].unwrap().answer_count, 2);
    assert_eq!(results[1].unwrap().rcode, 3);
    assert_eq!(results[1].unwrap().answer_count, 0);
}

#[test]
fn timeouts_retry_and_eventually_succeed() {
    let server = MockDnsServer::start([(
        "retry.example",
        vec![MockDnsAction::drop(), MockDnsAction::reply(3)],
    )]);

    let results = resolve_domains(&test_config(&server), &["retry.example".to_string()]).unwrap();

    assert_eq!(results, vec![true]);
}

#[test]
fn terminal_failures_collapse_to_unavailable() {
    let server = MockDnsServer::start([("drop.example", vec![MockDnsAction::drop()])]);

    let raw = resolve_domains_raw(&test_config(&server), &["drop.example".to_string()]).unwrap();
    let visible = resolve_domains(&test_config(&server), &["drop.example".to_string()]).unwrap();

    assert_eq!(raw, vec![None]);
    assert_eq!(visible, vec![false]);
}
