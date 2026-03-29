use crate::error::AppError;
use flate2::read::GzDecoder;
use memmap2::{Mmap, MmapOptions};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CACHE_MAGIC: &[u8; 4] = b"DGRP";
pub const CACHE_FORMAT_VERSION: u16 = 1;
pub const DOMAINS_PER_TLD: usize = 49_284;
const CACHE_MAX_AGE_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheMeta {
    pub format_version: u16,
    pub timestamp: i64,
    pub asset_url: String,
    pub asset_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheHeader {
    pub format_version: u16,
    pub timestamp: i64,
    pub tlds: Vec<String>,
    pub checksum: [u8; 32],
}

#[derive(Debug)]
enum BitmapStorage {
    Owned(Vec<u8>),
    Mapped { mmap: Mmap, offset: usize },
}

impl BitmapStorage {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Mapped { mmap, offset } => &mmap[*offset..],
        }
    }

    fn as_mut_slice(&mut self) -> Result<&mut [u8], AppError> {
        match self {
            Self::Owned(bytes) => Ok(bytes),
            Self::Mapped { .. } => Err(AppError::new("cannot mutate a memory-mapped cache file")),
        }
    }
}

#[derive(Debug)]
pub struct CacheFile {
    pub header: CacheHeader,
    bitmap: BitmapStorage,
}

#[derive(Debug)]
pub struct CacheStore {
    file: CacheFile,
    tld_index: HashMap<String, usize>,
    cache_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub cache_dir: PathBuf,
    pub asset_url: String,
    pub checksum_url: String,
}

impl CacheConfig {
    pub fn cache_bin_path(&self) -> PathBuf {
        self.cache_dir.join("cache.bin")
    }

    pub fn cache_meta_path(&self) -> PathBuf {
        self.cache_dir.join("cache.meta")
    }
}

impl CacheStore {
    pub async fn load_or_update(client: Client, config: CacheConfig) -> Result<Self, AppError> {
        fs::create_dir_all(&config.cache_dir)
            .map_err(|err| AppError::io("failed to create cache directory", err))?;

        match try_load_existing(&config) {
            Ok(Some(store)) => {
                if store.is_stale(&config) {
                    let client = client.clone();
                    let config = config.clone();
                    tokio::spawn(async move {
                        let _ = download_and_install(client, config).await;
                    });
                }
                Ok(store)
            }
            Ok(None) => download_and_install(client, config).await,
            Err(_) => download_and_install(client, config).await,
        }
    }

    pub fn header(&self) -> &CacheHeader {
        &self.file.header
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn tlds(&self) -> &[String] {
        &self.file.header.tlds
    }

    pub fn lookup(&self, domain: &str, tld: &str) -> Result<bool, AppError> {
        let Some(&tld_index) = self.tld_index.get(tld) else {
            return Err(AppError::new(format!(
                "unknown TLD '.{tld}' in cache lookup"
            )));
        };

        self.file.lookup_by_index(tld_index, domain)
    }

    fn from_file(file: CacheFile, cache_dir: PathBuf) -> Self {
        let tld_index = file
            .header
            .tlds
            .iter()
            .enumerate()
            .map(|(index, tld)| (tld.clone(), index))
            .collect();

        Self {
            file,
            tld_index,
            cache_dir,
        }
    }

    fn is_stale(&self, config: &CacheConfig) -> bool {
        let timestamp = read_cache_meta(&config.cache_meta_path())
            .map(|meta| meta.timestamp)
            .unwrap_or(self.file.header.timestamp);
        now_unix_seconds() - timestamp >= CACHE_MAX_AGE_SECONDS
    }
}

impl CacheFile {
    pub fn empty(tlds: Vec<String>, timestamp: i64) -> Self {
        Self {
            header: CacheHeader {
                format_version: CACHE_FORMAT_VERSION,
                timestamp,
                checksum: sha256_bytes(&vec![0; bitmap_len_for_tld_count(tlds.len())]),
                tlds,
            },
            bitmap: BitmapStorage::Owned(vec![]),
        }
        .with_zeroed_bitmap()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AppError> {
        let (header, bitmap_range) = parse_header(bytes)?;
        let bitmap = bytes[bitmap_range].to_vec();

        Ok(Self {
            header,
            bitmap: BitmapStorage::Owned(bitmap),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, AppError> {
        let file = File::open(path)
            .map_err(|err| AppError::io(format!("failed to open {}", path.display()), err))?;
        let mmap = unsafe {
            MmapOptions::new()
                .map(&file)
                .map_err(|err| AppError::io(format!("failed to map {}", path.display()), err))?
        };
        let (header, bitmap_range) = parse_header(&mmap)?;

        Ok(Self {
            header,
            bitmap: BitmapStorage::Mapped {
                mmap,
                offset: bitmap_range.start,
            },
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let bitmap = self.bitmap();
        let checksum = sha256_bytes(bitmap);

        let mut bytes = Vec::with_capacity(48 + bitmap.len() + self.header.tlds.len() * 4);
        bytes.extend_from_slice(CACHE_MAGIC);
        bytes.extend_from_slice(&self.header.format_version.to_le_bytes());
        bytes.extend_from_slice(&self.header.timestamp.to_le_bytes());
        bytes.extend_from_slice(&(self.header.tlds.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&checksum);

        for tld in &self.header.tlds {
            bytes.push(tld.len() as u8);
            bytes.extend_from_slice(tld.as_bytes());
        }

        bytes.extend_from_slice(bitmap);
        bytes
    }

    pub fn bitmap(&self) -> &[u8] {
        self.bitmap.as_slice()
    }

    pub fn set_available_by_index(
        &mut self,
        tld_index: usize,
        domain: &str,
        available: bool,
    ) -> Result<(), AppError> {
        let domain_index = domain_to_index(domain)? as usize;
        self.set_available_raw(tld_index, domain_index, available)
    }

    pub fn set_available_raw(
        &mut self,
        tld_index: usize,
        domain_index: usize,
        available: bool,
    ) -> Result<(), AppError> {
        if tld_index >= self.header.tlds.len() || domain_index >= DOMAINS_PER_TLD {
            return Err(AppError::new("cache bit position out of bounds"));
        }

        let bit_position = tld_index * DOMAINS_PER_TLD + domain_index;
        let bitmap = self.bitmap.as_mut_slice()?;
        set_bit(bitmap, bit_position, available);
        Ok(())
    }

    /// Copy a full TLD's bitmap slice from a source byte slice.
    /// Both src and dst must have the same DOMAINS_PER_TLD layout.
    pub fn copy_tld_bitmap(
        &mut self,
        dst_tld_index: usize,
        src_bitmap: &[u8],
        src_tld_index: usize,
    ) -> Result<(), AppError> {
        if dst_tld_index >= self.header.tlds.len() {
            return Err(AppError::new("destination TLD index out of bounds"));
        }

        let bits = DOMAINS_PER_TLD;
        let src_bit_start = src_tld_index * bits;
        let dst_bit_start = dst_tld_index * bits;

        // Fast path: copy aligned full bytes, then handle the trailing partial byte
        let full_bytes = bits / 8;
        let trailing_bits = bits % 8;

        let src_byte_start = src_bit_start / 8;
        let dst_byte_start = dst_bit_start / 8;

        let bitmap = self.bitmap.as_mut_slice()?;
        bitmap[dst_byte_start..dst_byte_start + full_bytes]
            .copy_from_slice(&src_bitmap[src_byte_start..src_byte_start + full_bytes]);

        if trailing_bits > 0 {
            let mask = !0u8 << (8 - trailing_bits);
            let src_byte = src_bitmap[src_byte_start + full_bytes];
            let dst = &mut bitmap[dst_byte_start + full_bytes];
            *dst = (src_byte & mask) | (*dst & !mask);
        }

        Ok(())
    }

    /// Recompute the header checksum from the current bitmap contents.
    /// Call this once after all bulk mutations are complete.
    pub fn finalize_checksum(&mut self) {
        self.header.checksum = sha256_bytes(self.bitmap.as_slice());
    }

    pub fn lookup_by_index(&self, tld_index: usize, domain: &str) -> Result<bool, AppError> {
        let domain_index = domain_to_index(domain)? as usize;
        Ok(self.is_available_raw(tld_index, domain_index))
    }

    pub fn is_available_raw(&self, tld_index: usize, domain_index: usize) -> bool {
        let bit_position = tld_index * DOMAINS_PER_TLD + domain_index;
        get_bit(self.bitmap(), bit_position)
    }

    fn with_zeroed_bitmap(mut self) -> Self {
        let bitmap = vec![0; bitmap_len_for_tld_count(self.header.tlds.len())];
        self.header.checksum = sha256_bytes(&bitmap);
        self.bitmap = BitmapStorage::Owned(bitmap);
        self
    }
}

pub fn all_short_domains() -> Vec<String> {
    let edge_chars = alpha_num_chars();
    let middle_chars = middle_chars();
    let mut domains = Vec::with_capacity(DOMAINS_PER_TLD);

    for first in &edge_chars {
        domains.push(first.to_string());
    }

    for first in &edge_chars {
        for second in &edge_chars {
            domains.push(format!("{first}{second}"));
        }
    }

    for first in &edge_chars {
        for middle in &middle_chars {
            for last in &edge_chars {
                domains.push(format!("{first}{middle}{last}"));
            }
        }
    }

    domains
}

pub fn domain_to_index(domain: &str) -> Result<u32, AppError> {
    let chars = domain.chars().collect::<Vec<_>>();
    let len = chars.len();

    if !(1..=3).contains(&len) {
        return Err(AppError::new(format!(
            "short cache domain '{domain}' must be 1 to 3 characters"
        )));
    }

    if !chars[0].is_ascii_lowercase() && !chars[0].is_ascii_digit() {
        return Err(AppError::new(format!("invalid cache domain '{domain}'")));
    }

    if !chars[len - 1].is_ascii_lowercase() && !chars[len - 1].is_ascii_digit() {
        return Err(AppError::new(format!("invalid cache domain '{domain}'")));
    }

    if len == 2 && chars[1] == '-' {
        return Err(AppError::new(format!("invalid cache domain '{domain}'")));
    }

    if len == 3 && !chars[1].is_ascii_lowercase() && !chars[1].is_ascii_digit() && chars[1] != '-' {
        return Err(AppError::new(format!("invalid cache domain '{domain}'")));
    }

    let offset = match len {
        1 => 0,
        2 => 36,
        3 => 36 + 1_296,
        _ => unreachable!(),
    };

    let index = match len {
        1 => char_to_val(chars[0], false),
        2 => char_to_val(chars[0], false) * 36 + char_to_val(chars[1], false),
        3 => {
            char_to_val(chars[0], false) * (37 * 36)
                + char_to_val(chars[1], true) * 36
                + char_to_val(chars[2], false)
        }
        _ => unreachable!(),
    };

    Ok(offset + index)
}

fn char_to_val(ch: char, allow_hyphen: bool) -> u32 {
    match ch {
        'a'..='z' => ch as u32 - 'a' as u32,
        '0'..='9' => 26 + (ch as u32 - '0' as u32),
        '-' if allow_hyphen => 36,
        _ => unreachable!("short domain should already be validated"),
    }
}

fn alpha_num_chars() -> Vec<char> {
    ('a'..='z').chain('0'..='9').collect()
}

fn middle_chars() -> Vec<char> {
    let mut chars = alpha_num_chars();
    chars.push('-');
    chars
}

fn parse_header(bytes: &[u8]) -> Result<(CacheHeader, std::ops::Range<usize>), AppError> {
    if bytes.len() < 48 || &bytes[..4] != CACHE_MAGIC {
        return Err(AppError::cache_integrity_failed());
    }

    let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if format_version != CACHE_FORMAT_VERSION {
        return Err(AppError::new(format!(
            "unsupported cache format version {format_version}"
        )));
    }

    let timestamp = i64::from_le_bytes(bytes[6..14].try_into().expect("slice length checked"));
    let tld_count = u16::from_le_bytes(bytes[14..16].try_into().expect("slice length checked"));
    let checksum: [u8; 32] = bytes[16..48].try_into().expect("slice length checked");

    let mut offset = 48usize;
    let mut tlds = Vec::with_capacity(tld_count as usize);
    for _ in 0..tld_count {
        if offset >= bytes.len() {
            return Err(AppError::cache_integrity_failed());
        }

        let len = bytes[offset] as usize;
        offset += 1;
        if offset + len > bytes.len() {
            return Err(AppError::cache_integrity_failed());
        }

        let tld = std::str::from_utf8(&bytes[offset..offset + len])
            .map_err(|_| AppError::cache_integrity_failed())?
            .to_string();
        offset += len;
        tlds.push(tld);
    }

    let bitmap_len = bitmap_len_for_tld_count(tlds.len());
    if bytes.len() != offset + bitmap_len {
        return Err(AppError::cache_integrity_failed());
    }

    let bitmap_range = offset..bytes.len();
    let actual_checksum = sha256_bytes(&bytes[bitmap_range.clone()]);
    if checksum != actual_checksum {
        return Err(AppError::cache_integrity_failed());
    }

    Ok((
        CacheHeader {
            format_version,
            timestamp,
            tlds,
            checksum,
        },
        bitmap_range,
    ))
}

fn bitmap_len_for_tld_count(tld_count: usize) -> usize {
    (tld_count * DOMAINS_PER_TLD).div_ceil(8)
}

fn get_bit(bitmap: &[u8], bit_position: usize) -> bool {
    let byte_offset = bit_position / 8;
    let bit_offset = bit_position % 8;
    (bitmap[byte_offset] >> (7 - bit_offset)) & 1 == 1
}

fn set_bit(bitmap: &mut [u8], bit_position: usize, available: bool) {
    let byte_offset = bit_position / 8;
    let bit_offset = bit_position % 8;
    let mask = 1 << (7 - bit_offset);

    if available {
        bitmap[byte_offset] |= mask;
    } else {
        bitmap[byte_offset] &= !mask;
    }
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_secs() as i64
}

fn try_load_existing(config: &CacheConfig) -> Result<Option<CacheStore>, AppError> {
    match CacheFile::from_path(&config.cache_bin_path()) {
        Ok(file) => Ok(Some(CacheStore::from_file(file, config.cache_dir.clone()))),
        Err(err) if config.cache_bin_path().exists() => {
            cleanup_cache(config);
            Err(err)
        }
        Err(err) => match std::fs::metadata(config.cache_bin_path()) {
            Ok(_) => Err(err),
            Err(meta_err) if meta_err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(meta_err) => Err(AppError::io("failed to stat cache file", meta_err)),
        },
    }
}

async fn download_and_install(client: Client, config: CacheConfig) -> Result<CacheStore, AppError> {
    let checksum_response = client
        .get(&config.checksum_url)
        .send()
        .await
        .map_err(|_| AppError::cache_download_failed())?;
    if !checksum_response.status().is_success() {
        return Err(AppError::cache_download_failed());
    }

    let checksum_text = checksum_response
        .text()
        .await
        .map_err(|_| AppError::cache_download_failed())?;
    let expected_asset_sha =
        parse_checksum(&checksum_text).ok_or_else(AppError::cache_download_failed)?;

    let asset_response = client
        .get(&config.asset_url)
        .send()
        .await
        .map_err(|_| AppError::cache_download_failed())?;
    if !asset_response.status().is_success() {
        return Err(AppError::cache_download_failed());
    }

    let compressed = asset_response
        .bytes()
        .await
        .map_err(|_| AppError::cache_download_failed())?
        .to_vec();
    let actual_asset_sha = hex_sha256(&compressed);
    if actual_asset_sha != expected_asset_sha {
        return Err(AppError::cache_integrity_failed());
    }

    let bytes = decompress_gzip(&compressed)?;
    let file = CacheFile::from_bytes(&bytes)?;
    let meta = CacheMeta {
        format_version: file.header.format_version,
        timestamp: file.header.timestamp,
        asset_url: config.asset_url.clone(),
        asset_sha256: expected_asset_sha,
    };

    write_atomically(&config.cache_bin_path(), &bytes)?;
    let meta_json = serde_json::to_vec(&meta)
        .map_err(|err| AppError::io("failed to serialize cache metadata", err))?;
    write_atomically(&config.cache_meta_path(), &meta_json)?;

    Ok(CacheStore::from_file(file, config.cache_dir.clone()))
}

fn parse_checksum(input: &str) -> Option<String> {
    input
        .split_whitespace()
        .next()
        .map(|value| value.to_ascii_lowercase())
}

fn decompress_gzip(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut decoder = GzDecoder::new(bytes);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|err| AppError::io("failed to decompress cache asset", err))?;
    Ok(output)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| AppError::io(format!("failed to create {}", parent.display()), err))?;
    }

    let tmp_path = path.with_file_name(format!(
        "{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cache.bin"),
        std::process::id()
    ));

    fs::write(&tmp_path, bytes)
        .map_err(|err| AppError::io(format!("failed to write {}", tmp_path.display()), err))?;

    replace_file(&tmp_path, path)
}

#[cfg(not(windows))]
fn replace_file(tmp_path: &Path, path: &Path) -> Result<(), AppError> {
    fs::rename(tmp_path, path)
        .map_err(|err| AppError::io(format!("failed to rename {}", tmp_path.display()), err))
}

#[cfg(windows)]
fn replace_file(tmp_path: &Path, path: &Path) -> Result<(), AppError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    if !path.exists() {
        return fs::rename(tmp_path, path)
            .map_err(|err| AppError::io(format!("failed to rename {}", tmp_path.display()), err));
    }

    let target = wide_path(path);
    let replacement = wide_path(tmp_path);
    let result = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };

    if result == 0 {
        return Err(AppError::io(
            format!("failed to replace {}", path.display()),
            std::io::Error::last_os_error(),
        ));
    }

    Ok(())
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn read_cache_meta(path: &Path) -> Option<CacheMeta> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn cleanup_cache(config: &CacheConfig) {
    let _ = fs::remove_file(config.cache_bin_path());
    let _ = fs::remove_file(config.cache_meta_path());
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
