use domaingrep::http::build_http_client;
use domaingrep::update::{maybe_start, UpdateConfig};
use std::fs;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn update_check_returns_notice_and_writes_timestamp() {
    let server = MockServer::start().await;
    let temp = tempdir().unwrap();

    Mock::given(method("GET"))
        .and(path("/latest"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("{\"tag_name\":\"v0.3.0\"}", "application/json"),
        )
        .mount(&server)
        .await;

    let handle = maybe_start(
        build_http_client(false).unwrap(),
        UpdateConfig {
            cache_dir: temp.path().to_path_buf(),
            api_url: format!("{}/latest", server.uri()),
            current_version: "0.2.0".to_string(),
        },
    )
    .unwrap()
    .unwrap();

    let notice = handle.await.unwrap().unwrap();
    assert_eq!(
        notice.render_lines()[0],
        "note: domaingrep v0.3.0 is available (current: v0.2.0)"
    );

    let timestamp = fs::read_to_string(temp.path().join("last_update_check")).unwrap();
    assert!(!timestamp.trim().is_empty());
}

#[tokio::test]
async fn update_check_is_skipped_when_last_check_is_fresh() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("last_update_check"), "99999999999").unwrap();

    let handle = maybe_start(
        build_http_client(false).unwrap(),
        UpdateConfig {
            cache_dir: temp.path().to_path_buf(),
            api_url: "http://127.0.0.1:9/latest".to_string(),
            current_version: "0.2.0".to_string(),
        },
    )
    .unwrap();

    assert!(handle.is_none());
}
