# domaingrep - Technical Specification

> Bulk domain availability search CLI tool.

**Version:** 0.1.0
**Last Updated:** 2026-03-29

---

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture](#2-architecture)
3. [CLI Interface](#3-cli-interface)
4. [Input Parsing & Validation](#4-input-parsing--validation)
5. [Domain Hack Detection](#5-domain-hack-detection)
6. [TLD Management](#6-tld-management)
7. [Cache System (1-3 Character Domains)](#7-cache-system-1-3-character-domains)
8. [Live DNS Resolution (4+ Character Domains)](#8-live-dns-resolution-4-character-domains)
9. [Output Format](#9-output-format)
10. [Auto-Update](#10-auto-update)
11. [Cache Builder (GitHub Actions)](#11-cache-builder-github-actions)
12. [Distribution & Installation](#12-distribution--installation)
13. [Project Structure](#13-project-structure)
14. [Testing Strategy](#14-testing-strategy)
15. [CI/CD Pipeline](#15-cicd-pipeline)
16. [Error Handling](#16-error-handling)
17. [Performance Targets](#17-performance-targets)
18. [Dependencies](#18-dependencies)

---

## 1. Overview

### 1.1 What is domaingrep?

`domaingrep` is a CLI tool that performs bulk domain name availability searches at extreme speed. It checks a given domain name (SLD) against all known TLDs and reports which combinations appear available for registration using fast DNS-based checks.

### 1.2 Core Philosophy

- **Instant speed** - 1-3 char domains via pre-built bitmap cache, 4+ via parallel DNS
- **Zero config** - Great defaults, no setup required
- **No interactive mode** - Pure non-interactive CLI, pipe-friendly
- **Auto-update** - CLI and cache stay current automatically
- **Cross-platform** - macOS, Linux, Windows

### 1.3 Key Innovation

**Two-tier availability check:**

| Domain Length | Method | Speed |
|---|---|---|
| 1-3 characters | Pre-built bitmap cache (daily) | O(1), instant |
| 4+ characters | Live DNS-over-HTTPS queries | ~2-5s parallel |

The bitmap cache stores a daily DNS-based availability snapshot for every possible 1-3 character domain across all TLDs. It is rebuilt daily by GitHub Actions and stored as a GitHub Release asset (~1-2 MB gzipped).

This is optimized for fast bulk candidate discovery rather than registrar-perfect accuracy. Reserved, premium, or registry-blocked names may still differ from the CLI's availability result.

---

## 2. Architecture

### 2.1 Execution Flow

```
domaingrep <input>
    |
    v
[1. Parse & Validate Input]
    |
    v
[2. Concurrent Operations] ----+---- [Cache freshness check (background)]
    |                          |
    |                          +---- [CLI version check (background)]
    |
    v
[3. Resolve Availability]
    |--- 1-3 char: Bitmap cache lookup (O(1))
    |--- 4+ char:  Parallel DoH queries to Cloudflare/Google DNS
    |
    v
[4. Domain Hack Detection]
    |--- Find all valid TLD suffix splits
    |--- Check availability for each hack
    |
    v
[5. Sort & Format Results]
    |--- Domain hacks at top
    |--- Then: TLD length ascending, same length by popularity
    |
    v
[6. Output to stdout]
    |--- Plain text (default) or JSON (--json)
```

### 2.2 Data Flow Diagram

```
GitHub Actions (daily cron)
    |
    v
[Cache Builder] ---> [GitHub Release Asset]
    |                      |
    |  (tld-list.com)      | (HTTP download)
    |                      |
    v                      v
[TLD List JSON]    [CLI: domaingrep]
                       |
                       +---> XDG_CACHE_HOME/domaingrep/
                        |         |- cache.bin          (bitmap, decompressed)
                        |         |- cache.meta         (metadata + asset checksum)
                        |         |- last_update_check  (timestamp)
                       |
                       +---> Cloudflare DoH (4+ char)
                       |         |- Primary: cloudflare-dns.com
                       |         |- Fallback: dns.google
                       |
                       v
                   [stdout: results]
```

---

## 3. CLI Interface

### 3.1 Usage

```
domaingrep [OPTIONS] <DOMAIN>
```

### 3.2 Positional Arguments

| Argument | Description |
|---|---|
| `<DOMAIN>` | Domain to search. Can be SLD only (`abc`) or SLD with TLD prefix (`abc.sh`). Single domain only. |

### 3.3 Flags & Options

| Flag | Short | Default | Description |
|---|---|---|---|
| `--all` | `-a` | `false` | Show unavailable domains too (default: available only) |
| `--json` | `-j` | `false` | Output as JSON (one object per line, NDJSON) |
| `--tld-len <RANGE>` | `-t` | (all) | Filter TLDs by length. Supports: `2` (exact), `2..5` (inclusive range), `..3` (up to 3), `4..` (4 and above) |
| `--limit <N>` | `-l` | (none) | Max number of rows emitted after filtering. Domain hacks count toward the total. All DNS requests are still made (no early termination). |
| `--color <WHEN>` | | `auto` | Color output: `auto`, `always`, `never`. Auto detects TTY. |
| `--help` | `-h` | | Show help message |
| `--version` | `-V` | | Show version |

### 3.4 Examples

```bash
# Search 'abc' across all TLDs (available only)
domaingrep abc

# TLD prefix match: 'sh' matches .sh, .shop, .show, etc.
domaingrep abc.sh

# Show all results including unavailable
domaingrep abc --all

# JSON output
domaingrep abc --json

# Only TLDs with 2-3 characters
domaingrep abc --tld-len 2..3

# Limit to first 10 available results
domaingrep abc --limit 10

# Domain hack detection: finds 'bun.sh' from input 'bunsh'
domaingrep bunsh

# Combine flags
domaingrep myapp.de --tld-len ..4 --limit 20 --json

# Multiple searches (documented approach)
domaingrep abc; domaingrep xyz
```

### 3.5 Exit Codes

| Code | Meaning |
|---|---|
| `0` | At least one available domain found (follows `grep` convention) |
| `1` | No available domains found |
| `2` | Error (invalid input, network failure, etc.) |

---

## 4. Input Parsing & Validation

### 4.1 Input Modes

The input `<DOMAIN>` is parsed into one of two modes:

**Mode A: SLD Only** (no dot in input)
```
domaingrep abc     -> SLD = "abc", TLDs = all
domaingrep bunsh   -> SLD = "bunsh", TLDs = all (+ domain hack detection)
```

**Mode B: SLD + TLD Prefix** (dot present)
```
domaingrep abc.sh  -> SLD = "abc", TLD filter = prefix "sh" (matches .sh, .shop, .show, ...)
domaingrep abc.com -> SLD = "abc", TLD filter = prefix "com" (matches .com, .community, ...)
domaingrep abc.    -> Trailing dot ignored, treated as "abc" (Mode A)
```

### 4.2 Validation Rules

1. **Allowed characters:** `[a-z0-9-]` (after lowercasing)
2. **Auto-lowercase:** `ABC` -> `abc` (silent, no warning)
3. **Trailing dot removal:** `abc.` -> `abc`
4. **Hyphen rules (LDH syntax + reserved punycode pattern):**
   - Cannot start with hyphen: `-abc` -> error
   - Cannot end with hyphen: `abc-` -> error
   - Cannot have hyphens at positions 3-4 (reserved for `xn--`/A-labels in IDN handling): `ab--c` -> error
5. **Length limits:**
   - SLD minimum: 1 character
   - SLD maximum: 63 characters
6. **Dot count:** at most one dot in `<DOMAIN>`; multi-label inputs like `abc.co.uk` -> error
7. **Empty input:** error
8. **Invalid characters:** error

### 4.3 Validation Error Format

All errors follow the Why / Where / How to Fix pattern:

```
error: invalid character '@' in domain 'abc@def'
  --> position 4
  = help: only letters (a-z), numbers (0-9), and hyphens (-) are allowed
```

```
error: domain cannot start with a hyphen
  --> '-abc'
  = help: remove the leading hyphen, e.g., 'abc'
```

```
error: domain too long (72 characters, max 63)
  --> 'aaaaaa...aaa'
  = help: domain labels must be 63 characters or fewer (RFC 1035)
```

---

## 5. Domain Hack Detection

### 5.1 Algorithm

Given input string `S` (no dot), find all suffixes of `S` that match a known TLD:

```
Input: "bunsh"

Scan suffixes:
  "bunsh" -> TLD "bunsh"? No
  "unsh"  -> TLD "unsh"?  No
  "nsh"   -> TLD "nsh"?   No
  "sh"    -> TLD "sh"?    Yes -> "bun.sh"
  "h"     -> TLD "h"?     No

Result: domain hack "bun.sh" detected
```

### 5.2 Rules

1. Only match against **valid, existing TLDs** from the TLD list
2. The SLD portion (before the matched TLD) must be **at least 1 character**
3. The SLD portion must be **valid** (same rules as 4.2)
4. **Priority:** shorter SLD first (i.e., longest matching TLD first)
   - `domaingrep openai` -> `ope.nai` (if .nai exists) before `opena.i` (if .i exists)
5. Domain hack results are placed **at the top** of the output, before regular results
6. Domain hack detection runs **in addition to** regular TLD search
   - `domaingrep bunsh` shows both `bun.sh` (hack) AND `bunsh.com`, `bunsh.net`, etc. (regular)
7. **Mode B (dot present): Domain hack detection is disabled**
   - `domaingrep bunsh.sh` does NOT detect `bun.sh` as a hack; it only searches `bunsh` with TLD prefix `sh`
8. **Availability check source for hacks:** The SLD from the hack split determines the check method:
   - SLD 1-3 chars -> bitmap cache lookup (e.g., `bun.sh` -> "bun" is 3 chars -> cache)
   - SLD 4+ chars -> live DNS query (e.g., `domaingre.p` -> "domaingre" is 9 chars -> DNS)
9. **--limit interaction:** Domain hack results count toward the `--limit` total
   - `--limit 5` with 3 hacks found -> shows 3 hacks + 2 regular results = 5 total

### 5.3 Data Structure for TLD Suffix Matching

Use a **reversed trie** built from the TLD list for efficient suffix matching:

```
TLD list: [sh, shop, show, com, co, ...]

Reversed trie:
  h -> s -> (match: "sh")
       -> p -> o -> (match: "shop")
  ...
```

For input "bunsh":
1. Reverse: "hsnub"
2. Walk the reversed trie from 'h'
3. Find match at depth 2: "sh" -> split is "bun" + "sh"

---

## 6. TLD Management

### 6.1 TLD Source

- **Primary:** `https://tld-list.com/df/tld-list-details.json`
- **Refresh:** Daily, during cache build (GitHub Actions)

### 6.2 TLD Filtering Criteria

From the tld-list.com JSON, include a TLD only if **all** conditions are met:

1. **ASCII only:** `punycode` field is `null` AND TLD key contains only `[a-z]` characters (exclude numeric TLDs that aren't purchasable)
2. **Not infrastructure:** `type` is not `"infrastructure"` (excludes `.arpa`)
3. **Publicly registrable:** TLD is available for public registration (not a brand-only TLD). Determined by **probe testing** during cache build:
   - Step 1: Query `nic.{tld}` for NS records. If no NS records exist, TLD is inactive -> exclude.
   - Step 2: Query `xyzzy-probe-test-{random}.{tld}` for NS records. If NXDOMAIN is returned, TLD supports public registration -> include. If NOERROR (wildcard) or SERVFAIL (after 3 retries), exclude.
   - This automatically filters out brand TLDs (e.g., `.google`, `.apple`) that don't allow public registration.

### 6.3 TLD Sorting

Results are sorted by:

1. **Primary:** TLD length ascending (`.io` before `.com` before `.shop`)
2. **Secondary (same length):** Popularity order from hardcoded list

### 6.4 Hardcoded Popularity List

Top ~50 TLDs in popularity order (used as secondary sort key):

```rust
const TLD_POPULARITY: &[&str] = &[
    // Length 2
    "io", "ai", "co", "me", "to", "sh", "cc", "tv", "is", "so",
    "im", "ly", "fm", "am", "it", "us", "uk", "de", "fr", "nl",
    "be", "at", "ch", "se", "no", "fi", "dk", "jp", "kr", "in",
    "ca", "au", "nz", "za", "br", "mx",
    // Length 3
    "com", "net", "org", "dev", "app", "xyz", "art", "fun", "icu", "top",
    "pro", "bio", "biz",
    // Length 4+
    "info", "club", "site", "tech", "shop", "blog", "design",
];
```

TLDs not in the popularity list are sorted alphabetically after the popular ones.

### 6.5 --tld-len Range Syntax

The `--tld-len` flag accepts a range:

| Input | Meaning | Parsed |
|---|---|---|
| `2` | Exactly length 2 | `min=2, max=2` |
| `2..5` | Length 2 to 5 (inclusive) | `min=2, max=5` |
| `..3` | Up to 3 (inclusive) | `min=1, max=3` |
| `4..` | 4 and above | `min=4, max=MAX` |

This is a user-friendly inclusive range syntax: `2..5` means 2 through 5, not Rust's exclusive upper-bound semantics.

---

## 7. Cache System (1-3 Character Domains)

### 7.1 Overview

All possible 1-3 character domains across all TLDs are pre-checked daily and stored in a compact bitmap. The bitmap is published as a GitHub Release asset and downloaded by the CLI on first run.

### 7.2 Domain Space

Valid characters: `[a-z0-9]` (36 chars) for start/end positions, `[a-z0-9-]` (37 chars) for middle positions.

- 1-char domains: only `[a-z0-9]` (no hyphen possible)
- 2-char domains: `[a-z0-9]` x `[a-z0-9]` (no room for middle hyphen)
- 3-char domains: `[a-z0-9]` x `[a-z0-9-]` x `[a-z0-9]` (middle position allows hyphen, e.g., `a-b`)

| Length | Calculation | Count |
|---|---|---|
| 1 char | 36 | 36 |
| 2 char | 36 x 36 | 1,296 |
| 3 char | 36 x 37 x 36 | 47,952 |
| **Total** | | **49,284** |

With ~1,200 TLDs (after filtering): 49,284 x 1,200 = ~59.1M domain-TLD pairs.

### 7.3 Bitmap Format

#### File Structure

```
+----------------------------------+
| Header (fixed size)              |
|   - Magic bytes (4B): "DGRP"    |
|   - Format version (2B): u16    |
|   - Timestamp (8B): i64 unix    |
|   - TLD count (2B): u16         |
|   - Checksum (32B): SHA-256     |
+----------------------------------+
| TLD Index Table                  |
|   - For each TLD:               |
|     - Length (1B): u8            |
|     - TLD string (variable)     |
+----------------------------------+
| Bitmap Data                      |
|   - Ordered by: TLD index, then |
|     domain index within TLD     |
|   - 1 bit per domain-TLD pair   |
|   - 1 = available               |
|   - 0 = unavailable             |
+----------------------------------+
```

#### Domain Index Calculation

Each domain maps to a deterministic index:

```rust
fn char_to_val(ch: char, allow_hyphen: bool) -> u32 {
    match ch {
        'a'..='z' => (ch as u32) - ('a' as u32),        // 0-25
        '0'..='9' => 26 + (ch as u32) - ('0' as u32),   // 26-35
        '-' if allow_hyphen => 36,                        // 36
        _ => unreachable!(), // validated before reaching here
    }
}

fn domain_to_index(domain: &str) -> u32 {
    let chars: Vec<char> = domain.chars().collect();
    let len = chars.len();

    // Offset for shorter domains
    let offset: u32 = match len {
        1 => 0,                               // 1-char: starts at 0
        2 => 36,                              // 2-char: starts after 36
        3 => 36 + 1_296,                      // 3-char: starts after 36 + 36*36
        _ => unreachable!(),
    };

    // Calculate position within same-length group
    let index = match len {
        1 => char_to_val(chars[0], false),
        2 => {
            char_to_val(chars[0], false) * 36
            + char_to_val(chars[1], false)
        }
        3 => {
            // first: 36 chars, middle: 37 chars (includes hyphen), last: 36 chars
            char_to_val(chars[0], false) * (37 * 36)
            + char_to_val(chars[1], true) * 36
            + char_to_val(chars[2], false)
        }
        _ => unreachable!(),
    };

    offset + index
}
```

#### Bit Lookup

```rust
fn is_available(cache: &[u8], tld_index: usize, domain: &str) -> bool {
    let domain_index = domain_to_index(domain) as usize;
    let domains_per_tld = 49_284; // total 1-3 char domains (36 + 1296 + 47952)
    let bit_position = tld_index * domains_per_tld + domain_index;
    let byte_offset = bit_position / 8;
    let bit_offset = bit_position % 8;
    (cache[byte_offset] >> (7 - bit_offset)) & 1 == 1
}
```

#### Size Estimation

- Bitmap: 49,284 domains x 1,200 TLDs = 59,140,800 bits = ~7.4 MB raw
- After gzip: estimated ~1-2 MB (bitmap data is highly compressible due to sparsity)

### 7.4 Local Cache Storage

**Location:** Determined by `dirs::cache_dir()` + `/domaingrep/`:
- **Linux:** `~/.cache/domaingrep/`
- **macOS:** `~/Library/Caches/domaingrep/`
- **Windows:** `%LOCALAPPDATA%/domaingrep/`

**Files:**
```
{cache_dir}/domaingrep/
  cache.bin            # decompressed bitmap cache used for lookups/mmap
  cache.meta           # metadata JSON: {"format_version":1,"timestamp":1711670400,"asset_url":"..."}
  last_update_check    # plain text Unix timestamp (e.g., "1711670400")
```

**cache.meta format (JSON):**
```json
{
  "format_version": 1,
  "timestamp": 1711670400,
  "asset_url": "https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.bin.gz",
  "asset_sha256": "a1b2c3..."
}
```

**last_update_check format:** Plain text file containing a single Unix timestamp (seconds). Example: `1711670400`

### 7.5 Cache Lifecycle

```
First Run:
  1. Check XDG_CACHE_HOME for existing cache
  2. No cache found -> download cache.bin.gz from GitHub Releases
  3. Verify SHA-256 checksum of downloaded asset
  4. Decompress to cache.bin, then memory-map
  5. Perform lookups

Subsequent Runs:
  1. Load local cache (instant)
  2. Check cache age from metadata
  3. If stale (>24h): use stale cache for query, AND start best-effort background refresh
  4. Background refresh: fetch new cache while query runs
  5. On download complete: verify checksum, write to temp file, atomic rename

No Local Cache + Download Failure:
  -> Error message + exit 2 (no fallback to DNS for short domains)

Background Refresh Failure:
  -> Keep using existing local cache
  -> Optionally print note to stderr in verbose/debug builds

Corrupt Cache:
  -> Delete local cache, re-download from GitHub Releases
  -> If re-download also fails: error + exit 2

Concurrent Instances:
  -> Downloads write to temp file (cache.bin.{pid}.tmp)
  -> Atomic rename to cache.bin on completion
  -> On Unix: rename() is atomic; on Windows: use ReplaceFile API
  -> Readers are unaffected (they memory-map the current cache.bin at startup)
```

### 7.6 Stale-While-Revalidate

The cache uses a stale-while-revalidate strategy:

```
Cache Age < 24h:  Use as-is (fresh)
Cache Age >= 24h: Use immediately (stale), start best-effort background refresh
Background task:  Download new cache -> verify checksum -> write to tmp -> atomic rename
```

The refresh runs concurrently with the main query but never delays result output or process exit. If the process exits before the refresh finishes, the refresh is abandoned and retried on a later run.

---

## 8. Live DNS Resolution (4+ Character Domains)

### 8.1 DNS-over-HTTPS Provider

- **Primary:** Cloudflare DoH (`https://cloudflare-dns.com/dns-query`)
- **Fallback:** Google DoH (`https://dns.google/resolve`)

### 8.2 Query Format

```
GET https://cloudflare-dns.com/dns-query?name=example.com&type=NS
Accept: application/dns-json
```

Response (JSON):
```json
{
  "Status": 0,
  "TC": false,
  "RD": true,
  "RA": true,
  "AD": false,
  "CD": false,
  "Question": [{"name": "example.com", "type": 2}],
  "Answer": [{"name": "example.com", "type": 2, "TTL": 21599, "data": "ns1.example.com."}]
}
```

### 8.3 Availability Determination

```
DNS Response Status:
  - NXDOMAIN (Status: 3)  -> available = true
  - NOERROR  (Status: 0)  -> available = false (domain exists)
  - SERVFAIL (Status: 2)  -> retry with fallback provider
  - Other                 -> retry with fallback provider
```

**Simplification:** NXDOMAIN = available, everything else = unavailable. No wildcard/parking IP detection.

This is an intentionally fast heuristic for bulk discovery and may not exactly match registrar-side purchasability.

### 8.4 HTTP Client Configuration

```rust
// reqwest client settings
let client = reqwest::Client::builder()
    .connect_timeout(Duration::from_secs(5))
    .timeout(Duration::from_secs(10))
    .user_agent(format!("domaingrep/{}", env!("CARGO_PKG_VERSION")))
    .http2_prior_knowledge()  // DoH servers support HTTP/2
    .pool_max_idle_per_host(50)
    .build()?;
```

| Setting | Value | Rationale |
|---|---|---|
| `connect_timeout` | 5 seconds | Fail fast on unreachable servers |
| `timeout` | 10 seconds | Max total request time including response |
| `User-Agent` | `domaingrep/{version}` | Identify traffic, good netizen behavior |
| HTTP version | HTTP/2 | Multiplexing reduces connection overhead |
| Pool idle | 50 per host | Reuse connections across parallel requests |

### 8.5 Concurrency Model

```rust
// Adaptive concurrency with backoff
struct DnsResolver {
    primary: CloudflareDoH,
    fallback: GoogleDoH,
    semaphore: Semaphore,       // limits concurrent requests
    initial_concurrency: usize, // 100
}
```

1. Start with 100 concurrent requests via `tokio::Semaphore`
2. If HTTP 429 (rate limit) from primary: switch to fallback for that request
3. If both fail: skip that TLD, count as failure
4. All TLDs are queried in parallel (no early termination even with `--limit`)
5. Domain hack results follow the same logic: SLD length determines source (1-3 char -> cache, 4+ char -> DNS)

### 8.6 Request Flow per TLD

```
1. Acquire semaphore permit
2. Send DoH query to Cloudflare
3. On success: return result
4. On failure (429/timeout/error):
   a. Send DoH query to Google DNS (fallback)
   b. On success: return result
   c. On failure: mark as skipped
5. Release semaphore permit
```

### 8.7 NS Record Query

The DNS query type is `NS` (not `A`):

- A registered domain is typically delegated with NS records
- An unregistered domain returns NXDOMAIN
- This avoids false negatives from domains without A/AAAA records

---

## 9. Output Format

### 9.1 Plain Text (Default)

```
$ domaingrep bunsh

bun.sh            # domain hack (at top)
bunsh.io
bunsh.co
bunsh.to
bunsh.com
bunsh.dev
bunsh.app
```

With `--all`:
```
$ domaingrep bunsh --all

  bun.sh            # domain hack, available
x bunsh.io
  bunsh.co
x bunsh.to
x bunsh.com
  bunsh.dev
  bunsh.app
```

### 9.2 Symbols & Colors

| Status | Symbol | Color |
|---|---|---|
| Available | ` ` (space, 2 chars indent) | Default/Green |
| Unavailable | `x` (1 char + space) | Dim/Gray |

- Colors are ANSI escape codes
- Auto-detected via `isatty()`: disabled when piped or redirected
- Override with `--color=always` or `--color=never`

When `--all` is **not** set, only available domains are shown (no symbols needed, just the domain name):

```
$ domaingrep bunsh

bun.sh
bunsh.co
bunsh.dev
bunsh.app
```

### 9.3 JSON Output (--json)

NDJSON format (one JSON object per line):

```json
{"domain":"bun.sh","available":true,"kind":"hack","method":"cache"}
{"domain":"bunsh.io","available":true,"kind":"regular","method":"dns"}
{"domain":"bunsh.com","available":false,"kind":"regular","method":"dns"}
```

- Fields: `domain`, `available`, `kind` (`hack` or `regular`), `method` (`cache` or `dns`)
- With `--all`: includes unavailable domains
- Without `--all`: only available domains (all `available: true`)

### 9.4 Output Order

1. **Domain hacks** (if any detected) - sorted by SLD length ascending
2. **Regular results** - sorted by:
   - Primary: TLD length ascending
   - Secondary: TLD popularity (hardcoded list)
   - Tertiary: Alphabetical

### 9.5 Pipe Behavior

When stdout is not a TTY (piped):
- No ANSI color codes
- No progress indicators
- Clean, parseable output
- One record per line in the selected format

---

## 10. Auto-Update

### 10.1 GitHub Repository

- **Owner/Repo:** `ysm-dev/domaingrep`
- **Cache Release URL:** `https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.bin.gz`
- **Cache Checksum URL:** `https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.sha256`
- **Latest Release API:** `https://api.github.com/repos/ysm-dev/domaingrep/releases/latest`

### 10.2 CLI Version Check

On runs where the previous check is missing or at least 24h old, a **best-effort non-blocking** background check is performed:

```
1. Read {cache_dir}/domaingrep/last_update_check
2. If <24h old: skip check
3. If >=24h or missing:
    a. Spawn async task (non-blocking)
    b. Query GitHub API: GET /repos/ysm-dev/domaingrep/releases/latest
    c. Compare version tag with current binary version
    d. If newer and the check completes before process exit: print notice to stderr after results
    e. Update last_update_check timestamp on successful completion
```

### 10.3 Update Notice

```
$ domaingrep abc
abc.sh
abc.io
...

note: domaingrep v0.3.0 is available (current: v0.2.0)
  -> brew upgrade domaingrep
  -> cargo install domaingrep
  -> curl -fsSL https://domaingrep.dev/install.sh | sh
```

- Printed to **stderr** (doesn't interfere with piped stdout)
- Best-effort and non-blocking (never delays result output or process exit)
- Maximum once per 24 hours

### 10.4 Cache Update

Cache is updated via stale-while-revalidate (see section 7.6). The background refresh runs concurrently with the main query and never blocks output or exit.

---

## 11. Cache Builder (GitHub Actions)

### 11.1 Overview

A daily GitHub Actions workflow rebuilds the bitmap cache by querying DNS for all 1-3 character domains across all TLDs.

### 11.2 Scale

- ~49,284 possible domains x ~1,200 TLDs = ~59.1M DNS queries
- Sharded across parallel matrix jobs for speed

Because this workload is large, the matrix sizes below are illustrative. Production builds may require self-hosted runners and/or a resolver source with explicit high-volume usage allowance.

### 11.3 Workflow Design

```yaml
# .github/workflows/cache-build.yml
name: Build Domain Cache

on:
  schedule:
    - cron: '0 2 * * *'  # Daily at 2:00 UTC
  workflow_dispatch:       # Manual trigger

jobs:
  fetch-tlds:
    runs-on: ubuntu-latest
    outputs:
      tld-groups: ${{ steps.split.outputs.groups }}
    steps:
      - uses: actions/checkout@v4
      - name: Fetch and filter TLD list
        id: split
        run: |
          # Download tld-list.com JSON
          # Filter: ASCII only, publicly registrable, no infrastructure
          # Split into groups of ~40 TLDs each for parallel processing
          # Output as JSON matrix

  scan:
    needs: fetch-tlds
    runs-on: ubuntu-latest
    strategy:
      matrix:
        group: ${{ fromJSON(needs.fetch-tlds.outputs.tld-groups) }}
      fail-fast: false
      max-parallel: 12
    steps:
      - uses: actions/checkout@v4
      - name: Build cache builder
        run: cargo build --release --bin cache-builder
      - name: Scan TLD group
        run: ./target/release/cache-builder scan --tlds '${{ matrix.group }}'
      - name: Upload partial bitmap
        uses: actions/upload-artifact@v4
        with:
          name: bitmap-${{ matrix.group }}
          path: partial-bitmap.bin

  merge:
    needs: scan
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Download all partial bitmaps
        uses: actions/download-artifact@v4
      - name: Merge bitmaps
        run: ./target/release/cache-builder merge --output cache.bin
      - name: Compress
        run: gzip -9 cache.bin
      - name: Generate checksum
        run: sha256sum cache.bin.gz > cache.sha256
      - name: Create/Update Release
        uses: softprops/action-gh-release@v2
        with:
          tag_name: cache-latest
          files: |
            cache.bin.gz
            cache.sha256
          prerelease: true
```

### 11.4 Cache Builder Binary

Located at `src/bin/cache_builder.rs`. Shares the same crate as the CLI.

**Commands:**

```
cache-builder fetch-tlds
  # Fetch TLD list from tld-list.com, filter, output JSON

cache-builder scan --tlds <TLD_LIST>
  # Scan all 1-3 char domains for given TLDs via Cloudflare DoH (NS record)
  # Output: partial bitmap file

cache-builder merge --output <PATH>
  # Merge all partial bitmap files into final cache.bin
  # Generates header with TLD index table
```

### 11.5 DNS Query Strategy (Cache Builder)

- **Provider:** Prototype with Cloudflare DoH JSON; production builder may need a dedicated/high-volume resolver source
- **Record type:** NS
- **Concurrency:** 100-200 concurrent requests per job (tuned conservatively to avoid rate limits)
- **Rate limiting:** Adaptive backoff on 429 responses
- **Retry:** 3 attempts per query before marking as unavailable

### 11.6 TLD List Freshness & Probe Testing

The cache builder fetches the TLD list from tld-list.com on every run, ensuring new TLDs are automatically included and removed TLDs are dropped.

**TLD probe test (during fetch-tlds job):**

```
For each TLD from tld-list.com (after ASCII/type filtering):
  1. Query NS for nic.{tld}
     - No NS records -> TLD inactive -> EXCLUDE
  2. Query NS for xyzzy-probe-test-{random_hex}.{tld}
     - NXDOMAIN -> public registration supported -> INCLUDE
     - NOERROR  -> wildcard DNS (brand TLD) -> EXCLUDE
     - SERVFAIL -> retry up to 3 times, then EXCLUDE
```

This eliminates the need for a manually maintained brand TLD exclusion list.

### 11.7 Cache Release Strategy

The cache is published under a fixed tag `cache-latest` that is overwritten daily:

```
Tag:    cache-latest (overwritten, not versioned)
Assets: cache.bin.gz, cache.sha256
URL:    https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.bin.gz
```

The GitHub Actions workflow deletes the existing `cache-latest` release and recreates it with new assets each day.

---

## 12. Distribution & Installation

### 12.1 Distribution Channels

| Channel | Package Name | Command |
|---|---|---|
| Homebrew | `domaingrep` | `brew install domaingrep` |
| Shell script | - | `curl -fsSL https://domaingrep.dev/install.sh \| sh` |
| npm | `domaingrep` | `npx domaingrep` / `npm i -g domaingrep` |
| Cargo | `domaingrep` | `cargo install domaingrep` |

### 12.2 npm Binary Distribution

Uses the `optionalDependencies` pattern (same as biome, napi-rs):

```
domaingrep                     (main package, JS wrapper)
  @domaingrep/darwin-arm64     (macOS Apple Silicon)
  @domaingrep/darwin-x64       (macOS Intel)
  @domaingrep/linux-arm64-gnu  (Linux ARM64)
  @domaingrep/linux-arm64-musl (Linux ARM64 musl)
  @domaingrep/linux-x64-gnu    (Linux x64)
  @domaingrep/linux-x64-musl   (Linux x64 musl)
  @domaingrep/win32-x64        (Windows x64)
```

The main `domaingrep` package contains a thin JS wrapper that locates and executes the platform-specific binary.

### 12.3 Shell Install Script

```bash
curl -fsSL https://domaingrep.dev/install.sh | sh
```

Installs to `~/.domaingrep/bin/domaingrep` and advises the user to add to PATH:

```
domaingrep was installed to ~/.domaingrep/bin/domaingrep
Add the following to your shell profile:
  export PATH="$HOME/.domaingrep/bin:$PATH"
```

### 12.4 Cross-Compilation Targets

| Target | OS | Arch |
|---|---|---|
| `x86_64-apple-darwin` | macOS | x64 |
| `aarch64-apple-darwin` | macOS | ARM64 |
| `x86_64-unknown-linux-musl` | Linux | x64 |
| `aarch64-unknown-linux-musl` | Linux | ARM64 |
| `x86_64-unknown-linux-gnu` | Linux | x64 |
| `x86_64-pc-windows-msvc` | Windows | x64 |

### 12.5 Binary Optimization

In `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

Expected binary size: ~3-5 MB.

---

## 13. Project Structure

### 13.1 Crate Layout

```
domaingrep/
  Cargo.toml
  src/
    main.rs              # Entry point, CLI arg parsing (clap)
    lib.rs               # Library root, re-exports
    cli.rs               # CLI argument definitions (clap derive)
    input.rs             # Input parsing & validation
    hack.rs              # Domain hack detection (trie-based)
    tld.rs               # TLD list management, filtering, sorting
    cache.rs             # Bitmap cache: load, lookup, download, verify
    dns.rs               # DoH resolver: Cloudflare + Google fallback
    output.rs            # Output formatting: plain text, JSON, colors
    update.rs            # Auto-update check logic
    error.rs             # Error types and formatting
    bin/
      cache_builder.rs   # Cache builder binary entry point
  tests/
    cli.rs              # End-to-end CLI tests
    cache.rs            # Cache download & lookup tests
    dns.rs              # DNS client tests (mostly mocked)
    hack.rs             # Domain hack detection tests
    output.rs           # Output format tests
    live_dns_smoke.rs   # Small live-network smoke suite
  data/
    tld_popularity.rs    # Hardcoded TLD popularity order
  .github/
    workflows/
      ci.yml             # PR: test + lint + build
      release.yml        # Release: cross-compile + publish
      cache-build.yml    # Daily: cache rebuild
```

### 13.2 Module Responsibilities

| Module | Responsibility |
|---|---|
| `cli.rs` | Clap derive structs, arg validation |
| `input.rs` | Parse input string, determine mode (SLD only / SLD+TLD prefix), validate characters |
| `hack.rs` | Build reversed trie from TLD list, find domain hack splits |
| `tld.rs` | Load TLD list from cache header, filter by length/prefix, sort by length+popularity |
| `cache.rs` | Download cache from GitHub Releases, SHA-256 verify, decompress, bitmap lookup, stale-while-revalidate |
| `dns.rs` | Cloudflare/Google DoH HTTP client, NS record query, adaptive concurrency, fallback logic |
| `output.rs` | Format results as plain text or JSON, ANSI color handling, TTY detection |
| `update.rs` | Check latest GitHub Release version, compare with current, print notice |
| `error.rs` | Custom error types, Why/Where/Fix formatting |

---

## 14. Testing Strategy

### 14.1 Philosophy: Test-Driven Development (TDD)

This project follows a TDD workflow. For every new module or feature:

1. **Write tests first** - Define the expected behavior as failing tests before writing any implementation code.
2. **Minimal implementation** - Write just enough code to make the tests pass.
3. **Refactor** - Clean up the implementation while keeping all tests green.

This applies to all layers: input validation, bitmap logic, DNS resolution, output formatting, and CLI integration. The test suite is the living specification of the system's behavior.

### 14.2 Approach

Layered test strategy: deterministic unit/integration tests for core logic, mocked HTTP tests for resolver behavior, and a small live DNS smoke suite for end-to-end confidence.

### 14.3 Test Categories

| Category | Description | Network Required |
|---|---|---|
| Input validation | Parse/validate various inputs | No |
| Domain hack | Trie construction, suffix matching | No |
| TLD filtering | Length filter, prefix match, sorting | No |
| Bitmap operations | Index calculation, bit read/write | No |
| Cache format | Serialize/deserialize, checksum verify | No |
| Output format | Plain text, JSON, color stripping | No |
| DNS resolution | Mocked DoH responses, fallback logic, timeout handling | No |
| Live DNS smoke | Real DoH queries against known domains | Yes |
| CLI end-to-end | Full pipeline from input to output (mostly fixture-based) | No |

### 14.4 CI Configuration

```yaml
- name: Run fast test suite
  run: cargo test --all-features --lib --bins --test cli --test cache --test dns --test hack --test output

- name: Run live DNS smoke tests
  run: cargo test --test live_dns_smoke -- --ignored
```

### 14.5 Test Data

Use well-known domains with stable availability:

```rust
// Always registered (unavailable)
const KNOWN_TAKEN: &[&str] = &["google.com", "github.com", "example.com"];

// Known NXDOMAIN (available) - use unlikely combinations
const KNOWN_AVAILABLE: &[&str] = &["xyzzy-test-domain-12345.com"];
```

### 14.6 Coverage Target

Target high confidence rather than literal 100% coverage. Aim for >=90% line coverage on core library modules over time, measured via `cargo-tarpaulin` or `cargo-llvm-cov`. The CI gate currently excludes `src/bin/cache_builder.rs`, which is not exercised by the automated suite yet, and requires >=75% overall line coverage on the remaining code.

---

## 15. CI/CD Pipeline

### 15.1 Workflow 1: CI (Pull Request)

**Trigger:** Pull request to main, push to main

```
Jobs:
  1. lint:     cargo clippy --all-targets -- -D warnings
  2. fmt:      cargo fmt --check
  3. test:     cargo test --all-features --lib --bins --test cli --test cache --test dns --test hack --test output
  4. smoke:    cargo test --test live_dns_smoke -- --ignored
  5. build:    cargo build --release (verify it compiles)
  6. coverage: cargo llvm-cov --ignore-filename-regex '(^|.*/)bin/cache_builder\.rs$' --fail-under-lines 75
```

### 15.2 Workflow 2: Release

**Trigger:** Git tag `v*`

```
Jobs:
  1. For each target (6 targets):
     a. Cross-compile: cargo build --release --target <target>
     b. Strip binary
     c. Create archive (tar.gz for unix, zip for windows)
  2. Create GitHub Release with all archives
  3. Publish to crates.io: cargo publish
  4. Publish npm packages (7 platform packages + main wrapper)
  5. Update Homebrew formula (tap repo)
  6. Generate install.sh with new version
```

### 15.3 Workflow 3: Cache Build (Daily)

**Trigger:** Daily cron (2:00 UTC), manual dispatch

See section 11.3 for full workflow details.

---

## 16. Error Handling

### 16.1 Error Format

All errors follow a consistent format printed to stderr:

```
error: <what went wrong>
  --> <where/context>
  = help: <how to fix>
```

### 16.2 Error Scenarios

| Scenario | Message | Exit Code |
|---|---|---|
| Invalid input characters | `error: invalid character '@' in domain 'ab@c'` | 2 |
| --limit 0 | `error: --limit must be at least 1` | 2 |
| Empty input | `error: no domain provided` | 2 |
| Domain too long | `error: domain too long (72 chars, max 63)` | 2 |
| No network | `error: network request failed: connection refused` | 2 |
| Cache download failed | `error: failed to download domain cache from GitHub Releases` | 2 |
| Cache checksum mismatch | `error: cache integrity check failed (SHA-256 mismatch)` | 2 |
| No available domains | stderr: `note: no available domains found for '{input}'` | 1 |
| Partial DNS failure | Results shown, then: `note: N TLDs could not be checked (DNS timeout)` on stderr | 0 or 1 |

### 16.3 Partial Failure

When some DNS queries fail during 4+ character domain checks:

1. Show all successful results normally
2. After results, print to stderr: `note: {N} of {total} TLDs could not be checked`
3. Exit code based on available domains found (0 or 1), not on failures

---

## 17. Performance Targets

| Metric | Target |
|---|---|
| 1-3 char domain (warm cache) | < 10ms |
| 1-3 char domain (cold cache, first download, typical broadband) | < 2s |
| 4+ char domain (all TLDs, healthy network) | typical < 5s |
| Binary startup time | < 5ms |
| Binary size | < 5 MB |
| Cache file size (gzipped) | < 2 MB |
| Memory usage | < 50 MB |

---

## 18. Dependencies

### 18.1 Rust Crate Dependencies

| Crate | Purpose |
|---|---|
| `clap` (derive) | CLI argument parsing |
| `tokio` | Async runtime |
| `reqwest` | HTTP client (DoH queries, cache download) |
| `serde` / `serde_json` | JSON parsing (DoH responses, TLD list) |
| `flate2` | gzip decompress (cache) |
| `sha2` | SHA-256 checksum verification |
| `dirs` | XDG/platform cache directory resolution |
| `atty` / `is-terminal` | TTY detection for color output |
| `anstream` / `anstyle` | ANSI color output (clap ecosystem) |

### 18.2 Minimal Dependency Philosophy

- Prefer crates already in the clap/tokio/reqwest dependency tree
- Avoid unnecessary dependencies that increase compile time or binary size
- No DNS wire format parsing crate needed (using JSON DoH)

---

## Appendix A: Bitmap Cache Wire Format

### Byte-level format

```
Offset  Size  Field
0       4     Magic: "DGRP" (0x44 0x47 0x52 0x50)
4       2     Format version: u16 LE (currently 1)
6       8     Build timestamp: i64 LE (Unix seconds)
14      2     TLD count: u16 LE
16      32    SHA-256 of bitmap data only
48      var   TLD index table: for each TLD:
                1 byte: TLD string length
                N bytes: TLD string (ASCII, no dot prefix)
var     var   Bitmap data:
                Ordered by: TLD index (0..tld_count), then domain index (0..49284)
                Total bits: tld_count * 49284
                Padded to byte boundary with zeros
```

### Example

For TLD list `["ai", "com", "io"]` and domain "abc":

```
TLD index: ai=0, com=1, io=2
Domain index of "abc":
  offset = 36 (1-char) + 1296 (2-char) = 1332
  index within 3-char group = a*(37*36) + b*36 + c
    = 0*(37*36) + 1*36 + 2 = 38
  domain_index = 1332 + 38 = 1370

Bit position for "abc.com": 1 * 49284 + 1370 = 50654
Byte offset: 50654 / 8 = 6331
Bit offset:  50654 % 8 = 6
```

For domain "a-b" (3-char with hyphen):

```
domain_index:
  offset = 1332
  index = 0*(37*36) + 36*36 + 1 = 1297  (hyphen is char_val 36, 'b' is 1)
  domain_index = 1332 + 1297 = 2629
```

## Appendix B: Cloudflare DoH Response Schema

### Request

```
GET https://cloudflare-dns.com/dns-query?name={domain}&type=NS
Accept: application/dns-json
```

### Response

```json
{
  "Status": 3,          // 0=NOERROR, 3=NXDOMAIN
  "TC": false,
  "RD": true,
  "RA": true,
  "AD": false,
  "CD": false,
  "Question": [
    {
      "name": "example.com",
      "type": 2           // NS=2
    }
  ],
  "Answer": [],           // empty for NXDOMAIN
  "Authority": []
}
```

### Status Code Mapping

| DNS Status | Code | Meaning | domaingrep Interpretation |
|---|---|---|---|
| NOERROR | 0 | Domain exists | unavailable |
| FORMERR | 1 | Format error | retry/skip |
| SERVFAIL | 2 | Server failure | retry with fallback |
| NXDOMAIN | 3 | Domain not found | **available** |
| NOTIMP | 4 | Not implemented | retry/skip |
| REFUSED | 5 | Query refused | retry with fallback |

## Appendix C: Supported TLD Types

| Type (tld-list.com) | Include? | Reason |
|---|---|---|
| `ccTLD` | Yes | Country-code TLDs (.io, .ai, .co, etc.) |
| `gTLD` | Partial | Generic TLDs, exclude brand-only TLDs |
| `grTLD` | Yes | Generic restricted (.biz, .name, .pro) |
| `sTLD` | Yes | Sponsored (.aero, .asia, .museum) |
| `infrastructure` | No | .arpa only, not registrable |
| IDN (punycode != null) | No | Non-ASCII TLDs excluded |
