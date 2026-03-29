#![allow(dead_code)]

use domaingrep::cache::{CacheFile, CacheMeta};
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn cache_fixture(tlds: &[&str], available: &[(&str, &str)]) -> CacheFile {
    cache_fixture_at(tlds, available, now_unix_seconds())
}

pub fn cache_fixture_at(tlds: &[&str], available: &[(&str, &str)], timestamp: i64) -> CacheFile {
    let mut file = CacheFile::empty(
        tlds.iter().map(|tld| (*tld).to_string()).collect(),
        timestamp,
    );

    for (domain, tld) in available {
        let tld_index = file
            .header
            .tlds
            .iter()
            .position(|candidate| candidate == tld)
            .expect("test TLD should exist in fixture");
        file.set_available_by_index(tld_index, domain, true)
            .expect("fixture domain should be valid");
    }

    file
}

pub fn write_local_cache(dir: &Path, tlds: &[&str], available: &[(&str, &str)]) {
    fs::create_dir_all(dir).expect("cache dir should be creatable");
    let file = cache_fixture(tlds, available);

    fs::write(dir.join("cache.bin"), file.to_bytes()).expect("cache.bin should write");
    write_cache_meta(
        dir,
        file.header.timestamp,
        "http://example.test/cache.bin.gz",
        "deadbeef",
    );
}

pub fn write_cache_meta(dir: &Path, timestamp: i64, asset_url: &str, asset_sha256: &str) {
    fs::write(
        dir.join("cache.meta"),
        serde_json::to_vec(&CacheMeta {
            format_version: 1,
            timestamp,
            asset_url: asset_url.to_string(),
            asset_sha256: asset_sha256.to_string(),
        })
        .expect("cache meta should serialize"),
    )
    .expect("cache.meta should write");
}

pub fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_secs() as i64
}

pub fn gzip_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(bytes)
        .expect("gzip encoder should accept bytes");
    encoder.finish().expect("gzip encoder should finish")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}
