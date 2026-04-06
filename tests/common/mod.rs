#![allow(dead_code)]

use domaingrep::cache::{CacheFile, CacheMeta};
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MockDnsReply {
    pub rcode: u8,
    pub answer_count: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockDnsAction {
    Reply(MockDnsReply),
    Drop,
}

impl MockDnsAction {
    pub fn reply(rcode: u8) -> Self {
        Self::Reply(MockDnsReply {
            rcode,
            answer_count: 0,
        })
    }

    pub fn reply_with_answers(rcode: u8, answer_count: u16) -> Self {
        Self::Reply(MockDnsReply {
            rcode,
            answer_count,
        })
    }

    pub fn drop() -> Self {
        Self::Drop
    }
}

pub struct MockDnsServer {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockDnsServer {
    pub fn start<const N: usize>(entries: [(&str, Vec<MockDnsAction>); N]) -> Self {
        Self::start_with_default(entries, MockDnsAction::Drop)
    }

    pub fn start_with_default<const N: usize>(
        entries: [(&str, Vec<MockDnsAction>); N],
        default_action: MockDnsAction,
    ) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("mock DNS socket should bind");
        socket
            .set_nonblocking(true)
            .expect("mock DNS socket should be non-blocking");

        let addr = socket
            .local_addr()
            .expect("mock DNS socket should have address");
        let shutdown = Arc::new(AtomicBool::new(false));
        let responses = Arc::new(Mutex::new(
            entries
                .into_iter()
                .map(|(name, actions)| {
                    (
                        name.to_string(),
                        actions.into_iter().collect::<VecDeque<_>>(),
                    )
                })
                .collect::<HashMap<_, _>>(),
        ));

        let thread_shutdown = shutdown.clone();
        let thread_responses = responses.clone();
        let handle = thread::spawn(move || {
            let mut buffer = [0u8; 512];

            while !thread_shutdown.load(Ordering::Relaxed) {
                match socket.recv_from(&mut buffer) {
                    Ok((len, peer)) => {
                        let Some((id, name)) = parse_query(&buffer[..len]) else {
                            continue;
                        };

                        let action = {
                            let mut responses = thread_responses
                                .lock()
                                .expect("mock DNS responses mutex should not be poisoned");
                            match responses.get_mut(&name) {
                                Some(queue) if queue.len() > 1 => {
                                    queue.pop_front().unwrap_or_else(MockDnsAction::drop)
                                }
                                Some(queue) => {
                                    queue.front().cloned().unwrap_or_else(MockDnsAction::drop)
                                }
                                None => default_action.clone(),
                            }
                        };

                        if let MockDnsAction::Reply(reply) = action {
                            let payload = build_response(id, reply.rcode, reply.answer_count);
                            let _ = socket.send_to(&payload, peer);
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            addr,
            shutdown,
            handle: Some(handle),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for MockDnsServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn write_resolvers_file(path: &Path, addrs: &[SocketAddr]) {
    let contents = addrs
        .iter()
        .map(SocketAddr::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, format!("{contents}\n")).expect("resolvers file should write");
}

fn parse_query(packet: &[u8]) -> Option<(u16, String)> {
    if packet.len() < 12 {
        return None;
    }

    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let mut offset = 12usize;
    let mut labels = Vec::new();

    loop {
        let len = *packet.get(offset)? as usize;
        offset += 1;
        if len == 0 {
            break;
        }

        let label = std::str::from_utf8(packet.get(offset..offset + len)?).ok()?;
        labels.push(label.to_string());
        offset += len;
    }

    Some((id, labels.join(".")))
}

fn build_response(id: u16, rcode: u8, answer_count: u16) -> [u8; 12] {
    let mut packet = [0u8; 12];
    packet[..2].copy_from_slice(&id.to_be_bytes());
    packet[2] = 0x81;
    packet[3] = 0x80 | (rcode & 0x0f);
    packet[4..6].copy_from_slice(&1u16.to_be_bytes());
    packet[6..8].copy_from_slice(&answer_count.to_be_bytes());
    packet[8..10].copy_from_slice(&0u16.to_be_bytes());
    packet[10..12].copy_from_slice(&0u16.to_be_bytes());
    packet
}
