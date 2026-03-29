use domaingrep::cli::ColorWhen;
use domaingrep::output::{render, CheckMethod, DomainResult, OutputOptions, ResultKind};

#[test]
fn renders_plain_text_available_only() {
    let results = vec![
        DomainResult {
            domain: "bun.sh".to_string(),
            available: true,
            kind: ResultKind::Hack,
            method: CheckMethod::Cache,
        },
        DomainResult {
            domain: "bunsh.com".to_string(),
            available: false,
            kind: ResultKind::Regular,
            method: CheckMethod::Dns,
        },
    ];

    let output = render(
        &results,
        OutputOptions {
            json: false,
            show_all: false,
            color: ColorWhen::Never,
        },
    );

    assert_eq!(output, "bun.sh\n");
}

#[test]
fn renders_plain_text_with_symbols() {
    let results = vec![
        DomainResult {
            domain: "bun.sh".to_string(),
            available: true,
            kind: ResultKind::Hack,
            method: CheckMethod::Cache,
        },
        DomainResult {
            domain: "bunsh.com".to_string(),
            available: false,
            kind: ResultKind::Regular,
            method: CheckMethod::Dns,
        },
    ];

    let output = render(
        &results,
        OutputOptions {
            json: false,
            show_all: true,
            color: ColorWhen::Never,
        },
    );

    assert_eq!(output, "  bun.sh\nx bunsh.com\n");
}

#[test]
fn renders_ndjson() {
    let results = vec![
        DomainResult {
            domain: "bun.sh".to_string(),
            available: true,
            kind: ResultKind::Hack,
            method: CheckMethod::Cache,
        },
        DomainResult {
            domain: "bunsh.com".to_string(),
            available: false,
            kind: ResultKind::Regular,
            method: CheckMethod::Dns,
        },
    ];

    let output = render(
        &results,
        OutputOptions {
            json: true,
            show_all: true,
            color: ColorWhen::Never,
        },
    );

    assert_eq!(
        output,
        concat!(
            "{\"domain\":\"bun.sh\",\"available\":true,\"kind\":\"hack\",\"method\":\"cache\"}\n",
            "{\"domain\":\"bunsh.com\",\"available\":false,\"kind\":\"regular\",\"method\":\"dns\"}\n"
        )
    );
}
