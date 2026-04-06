use domaingrep::resolve::{resolve_domains, ResolveConfig};

#[test]
#[ignore]
fn known_registered_domain_is_unavailable() {
    let results = resolve_domains(&ResolveConfig::default(), &["google.com".to_string()]).unwrap();
    assert_eq!(results, vec![false]);
}

#[test]
#[ignore]
fn unlikely_domain_is_available() {
    let results = resolve_domains(
        &ResolveConfig::default(),
        &["xyzzy-test-domain-12345.com".to_string()],
    )
    .unwrap();
    assert_eq!(results, vec![true]);
}
