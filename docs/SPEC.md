# domaingrep - Technical Specification

> Bulk domain availability search CLI tool.

**Version:** 0.2.1
**Last Updated:** 2026-04-06

---

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture](#2-architecture)
3. [CLI Interface](#3-cli-interface)
4. [Input Parsing and Validation](#4-input-parsing-and-validation)
5. [Domain Hack Detection](#5-domain-hack-detection)
6. [TLD Management](#6-tld-management)
7. [Cache System (1-3 Character Domains)](#7-cache-system-1-3-character-domains)
8. [UDP DNS Resolution (4+ Character Domains)](#8-udp-dns-resolution-4-character-domains)
9. [Output Format](#9-output-format)
10. [Auto-Update](#10-auto-update)
11. [Cache Builder and GitHub Actions](#11-cache-builder-and-github-actions)
12. [Distribution and Installation](#12-distribution-and-installation)
13. [Project Structure](#13-project-structure)
14. [Testing Strategy](#14-testing-strategy)
15. [CI/CD Pipeline](#15-cicd-pipeline)
16. [Error Handling](#16-error-handling)
17. [Performance Notes](#17-performance-notes)
18. [Dependencies](#18-dependencies)

---

## 1. Overview

### 1.1 What domaingrep does

`domaingrep` checks a single input label across many TLDs and reports domains that appear available for registration.

The implementation is intentionally optimized for bulk candidate discovery, not registrar-perfect availability. Reserved, premium, or policy-blocked names may still differ from the CLI result.

### 1.2 Two-tier resolution strategy

| Domain length | Method | Source |
|---|---|---|
| 1-3 characters | Bitmap cache lookup | Local `cache.bin` |
| 4+ characters | Live UDP DNS NS query | Shared Rust resolver engine |

For domain hacks, the method is chosen by the hack SLD length, not by the original input length.

### 1.3 External behavior

- Non-interactive CLI only
- Plain text by default, NDJSON with `--json`
- Available-only by default, full listing with `--all`
- Background cache refresh and update check are best-effort and never block the main query
- DNS query failures are collapsed to `unavailable` rather than surfaced as partial-output skips

---

## 2. Architecture

### 2.1 Execution flow

```text
domaingrep <DOMAIN>
    |
    v
[1] Parse and validate input
    |
    v
[2] Start background work
    |--- cache freshness check (best effort)
    |--- CLI update check (best effort)
    |
    v
[3] Load cache
    |
    v
[4] Resolve domain hacks (mode A only)
    |--- 1-3 char hack SLD -> bitmap cache
    |--- 4+ char hack SLD  -> UDP DNS resolver
    |
    v
[5] Resolve regular TLD matches
    |--- 1-3 char SLD -> bitmap cache
    |--- 4+ char SLD  -> UDP DNS resolver
    |
    v
[6] Sort and format results
    |
    v
[7] Print stdout, then optional stderr notes
```

### 2.2 Current resolver architecture

The live resolver is a Rust-native UDP stub resolver shared by:

- the CLI for 4+ character domains
- the cache builder for 1-3 character cache generation
- the TLD probe step used by `cache-builder fetch-tlds`

The resolver currently uses:

- DNS wire-format packet construction in `src/resolve/wire.rs`
- non-blocking UDP sockets built with `socket2`
- `mio::Poll` for readiness polling
- a direct-indexed lookup slab keyed by DNS transaction ID
- a timing wheel for retries and timeouts

It does not use DoH, `reqwest`, or an external `massdns` subprocess.

### 2.3 Runtime configuration via environment

These environment variables are supported by the CLI runtime:

| Variable | Meaning |
|---|---|
| `DOMAINGREP_CACHE_DIR` | Override cache directory |
| `DOMAINGREP_CACHE_URL` | Override cache asset URL |
| `DOMAINGREP_CACHE_CHECKSUM_URL` | Override checksum URL |
| `DOMAINGREP_UPDATE_API_URL` | Override GitHub latest-release API URL |
| `DOMAINGREP_RESOLVERS` | Override resolver list (comma or whitespace separated) |
| `DOMAINGREP_RESOLVE_CONCURRENCY` | Override live resolver concurrency |
| `DOMAINGREP_RESOLVE_TIMEOUT_MS` | Override per-attempt timeout in milliseconds |
| `DOMAINGREP_RESOLVE_ATTEMPTS` | Override max attempts per domain |
| `DOMAINGREP_RESOLVE_SOCKET_COUNT` | Override number of UDP sockets |
| `DOMAINGREP_DISABLE_UPDATE` | Disable background version check |

`DOMAINGREP_RESOLVERS` accepts IPs or socket addresses such as `1.1.1.1`, `8.8.8.8:53`, or mixed comma/whitespace-separated lists.

---

## 3. CLI Interface

### 3.1 Usage

```text
domaingrep [OPTIONS] <DOMAIN>
```

### 3.2 Positional argument

| Argument | Description |
|---|---|
| `<DOMAIN>` | Search target. Supports `abc` or `abc.sh`. Exactly one domain only. |

### 3.3 Flags and options

| Flag | Short | Default | Description |
|---|---|---|---|
| `--all` | `-a` | `false` | Show unavailable results too |
| `--json` | `-j` | `false` | Emit NDJSON |
| `--tld-len <RANGE>` | `-t` | all | Filter TLDs by length: `2`, `2..5`, `..3`, `4..` |
| `--limit <N>` | `-l` | `25` on terminal, none otherwise | Maximum rows emitted after filtering. `0` means unlimited. |
| `--color <WHEN>` | | `auto` | `auto`, `always`, `never` |
| `--help` | `-h` | | Show help |
| `--version` | `-V` | | Show version |

### 3.4 Exit codes

| Code | Meaning |
|---|---|
| `0` | At least one available result was emitted |
| `1` | No available results |
| `2` | Invalid input, cache/config/network/bootstrap error |

Important: per-domain DNS timeouts and inconclusive responses do not produce exit code `2`. They collapse to `unavailable` and can contribute to exit code `1`.

---

## 4. Input Parsing and Validation

### 4.1 Modes

**Mode A: SLD only**

```text
domaingrep abc
```

Parsed as:

- `sld = "abc"`
- `tld_prefix = None`
- domain hack detection enabled

**Mode B: SLD + TLD prefix**

```text
domaingrep abc.sh
```

Parsed as:

- `sld = "abc"`
- `tld_prefix = Some("sh")`
- domain hack detection disabled

### 4.2 Normalization rules

1. Input is lowercased silently.
2. A single trailing dot is removed before dot-count validation.
3. After trailing-dot removal, at most one dot is allowed.

Examples:

```text
ABC      -> abc
abc.     -> abc
abc.co.  -> sld=abc, prefix=co
abc.co.uk -> error
```

### 4.3 Label validation rules

For both the SLD and the optional TLD prefix:

1. Allowed characters: `[a-z0-9-]`
2. Minimum length: 1
3. Maximum length: 63
4. Cannot start with `-`
5. Cannot end with `-`
6. Cannot contain `--` at positions 3-4

### 4.4 Error format

Errors use this shape:

```text
error: <message>
  --> <optional context>
  = help: <optional suggestion>
```

The `-->` line is emitted only when the implementation has a useful location/context string.

Examples:

```text
error: invalid character '@' in domain 'ab@c'
  --> position 3
  = help: only letters (a-z), numbers (0-9), and hyphens (-) are allowed
```

```text
error: invalid --tld-len range 'x'
  = help: use '2', '2..5', '..3', or '4..'
```

---

## 5. Domain Hack Detection

### 5.1 Behavior

For mode A inputs, domaingrep scans all suffixes of the input and matches them against the known TLD set.

Example:

```text
Input: bunsh
Matches: bun.sh
```

### 5.2 Rules

1. Only known filtered TLDs are considered.
2. The SLD part before the TLD must be at least 1 character.
3. The derived SLD must pass the same label validation rules as normal input.
4. Results are sorted by SLD length ascending.
   This means longer matching TLDs appear first.
5. Domain hack results are always emitted before regular results.
6. In mode B (`abc.sh`), hack detection is disabled.
7. Hack results count toward `--limit`.

### 5.3 Resolution method

| Hack SLD length | Method |
|---|---|
| 1-3 | Cache lookup |
| 4+ | Live UDP DNS resolution |

---

## 6. TLD Management

### 6.1 Source

- HTTP source: `https://tld-list.com/df/tld-list-details.json`

### 6.2 Filtering rules

TLDs are included only if all of the following hold:

1. ASCII lowercase key only
2. `punycode` is `null`
3. `type != "infrastructure"`
4. Public registration probe passes

### 6.3 Public registration probe

The builder uses the shared UDP resolver in two passes:

1. Query `nic.{tld}` with `NS`
   - include only if `RCODE == NOERROR` and `answer_count > 0`
2. Query `xyzzy-probe-test-{random}.{tld}` with `NS`
   - include only if `RCODE == NXDOMAIN`

Any timeout or inconclusive result excludes the TLD.

### 6.4 Sorting

Regular results are sorted by:

1. TLD length ascending
2. Hardcoded popularity order
3. Alphabetical order

### 6.5 `--tld-len`

Supported syntax:

| Input | Meaning |
|---|---|
| `2` | exactly 2 |
| `2..5` | inclusive range |
| `..3` | up to 3 |
| `4..` | 4 and above |

---

## 7. Cache System (1-3 Character Domains)

### 7.1 Scope

The cache stores availability bits for all valid 1-3 character labels across the filtered TLD set.

### 7.2 Domain space

| Length | Count |
|---|---|
| 1 | 36 |
| 2 | 1,296 |
| 3 | 47,952 |
| Total | 49,284 |

Character rules used for indexing:

- edge positions: `[a-z0-9]`
- middle position of 3-char labels: `[a-z0-9-]`

### 7.3 File format

`cache.bin` contains:

1. magic bytes: `DGRP`
2. format version: `u16 LE`
3. build timestamp: `i64 LE`
4. TLD count: `u16 LE`
5. SHA-256 of bitmap payload
6. variable-length TLD index table
7. bitmap payload

The bitmap is ordered by:

1. TLD index
2. domain index within that TLD

`1` means available, `0` means unavailable.

### 7.4 Local storage

Default cache directory:

- Linux: `~/.cache/domaingrep/`
- macOS: `~/Library/Caches/domaingrep/`
- Windows: `%LOCALAPPDATA%/domaingrep/`

Files:

```text
cache.bin
cache.meta
last_update_check
```

### 7.5 Lifecycle

1. If `cache.bin` exists and parses, use it immediately.
2. If stale (>=24h), continue using it and start a background refresh.
3. If missing or corrupt, download `cache.bin.gz` and `cache.sha256`.
4. Verify checksum, decompress, and atomically replace local files.
5. If no local cache exists and download fails, command exits with code `2`.

Short-domain resolution never falls back to live DNS if the cache cannot be bootstrapped.

---

## 8. UDP DNS Resolution (4+ Character Domains)

### 8.1 Query model

- Record type: `NS`
- Transport: UDP to configured recursive resolvers
- No DoH
- No TCP fallback

### 8.2 Default resolver list

Default embedded resolvers:

```text
1.1.1.1
1.0.0.1
8.8.8.8
8.8.4.4
9.9.9.9
149.112.112.112
208.67.222.222
208.67.220.220
```

### 8.3 Classification

The shared classification rule is:

| Result | Meaning |
|---|---|
| `NXDOMAIN` | available |
| `NOERROR` | unavailable |
| other RCODE | retry, then unavailable if attempts exhausted |
| timeout / no response | retry, then unavailable if attempts exhausted |

There is no external `unknown` state.

### 8.4 Resolver defaults

CLI defaults:

- concurrency: `1000`
- timeout: `500ms`
- max attempts: `4`
- socket count: `1`

Builder defaults:

- concurrency: `10000`
- timeout: `500ms`
- max attempts: `4`
- socket count: `4` on Linux, otherwise `1`

### 8.5 Internal engine behavior

The live resolver:

1. builds DNS wire-format NS query packets
2. allocates a unique transaction ID
3. sends packets over non-blocking UDP sockets
4. tracks in-flight lookups in a slab indexed by transaction ID
5. retries timed-out or inconclusive lookups via a timing wheel
6. returns definitive `rcode`/`answer_count` or `None`

For CLI-visible output, `None` is treated as unavailable.

---

## 9. Output Format

### 9.1 Plain text

Default output emits only available results:

```text
bun.sh
bunsh.io
bunsh.dev
```

With `--all`:

```text
  bun.sh
x bunsh.io
  bunsh.dev
```

### 9.2 JSON

`--json` emits NDJSON, one object per line:

```json
{"domain":"bun.sh","available":true,"kind":"hack","method":"cache"}
{"domain":"bunsh.io","available":false,"kind":"regular","method":"dns"}
```

Fields:

- `domain`
- `available`
- `kind`: `hack` or `regular`
- `method`: `cache` or `dns`

### 9.3 Ordering

1. All hack results first
2. Then regular results sorted by TLD length, popularity, alphabetically

### 9.4 `--limit`

`--limit` is applied after visibility filtering:

- without `--all`: after dropping unavailable results
- with `--all`: after including both available and unavailable results
- `--limit 0` disables truncation
- when stdout is a terminal, plain text output defaults to `25` rows if `--limit` is omitted
- when stdout is not a terminal, or when `--json` is used, omitted `--limit` means unlimited

All DNS work is still performed before truncation.

### 9.5 stderr notes

Current stderr notes:

- `note: no available domains found for '{input}'`
- `note: {remaining} more domains not shown (showing {shown} of {total}; use --limit 0 to show all)` when plain text output is truncated
- update notice, if background update check finishes before process exit

The implementation does not emit a partial-DNS-failure note.

---

## 10. Auto-Update

### 10.1 Version check

When `last_update_check` is missing or at least 24 hours old:

1. start a background GitHub API request
2. fetch latest release metadata
3. compare current version with release tag
4. if newer and ready before exit, print a stderr note
5. update `last_update_check`

### 10.2 Guarantees

- best effort only
- never delays main output
- maximum once per 24 hours

---

## 11. Cache Builder and GitHub Actions

### 11.1 Commands

The `cache-builder` binary exposes three commands:

```text
cache-builder fetch-tlds
cache-builder scan --tlds <...>
cache-builder merge --output <PATH>
```

### 11.2 `fetch-tlds`

Responsibilities:

1. fetch TLD JSON from the source URL
2. probe public registrability using the shared resolver engine
3. sort and split TLDs into groups
4. print JSON matrix output for GitHub Actions

### 11.3 `scan`

Responsibilities:

1. generate all short domains
2. resolve `{domain}.{tld}` with shared UDP DNS engine in chunks
3. set bitmap bits for available results
4. write `partial-bitmap.bin`

This command no longer depends on `massdns`.

### 11.4 `merge`

Responsibilities:

1. collect all partial bitmap files
2. merge per-TLD slices into a single cache file
3. write final `cache.bin`

### 11.5 GitHub Actions workflow

Current cache build workflow:

1. build `cache-builder`
2. run `fetch-tlds`
3. matrix-scan TLD groups with `cache-builder scan`
4. merge partial bitmaps
5. gzip and checksum the final cache
6. publish `cache-latest` release assets

---

## 12. Distribution and Installation

The repository includes release automation for:

- GitHub release archives
- crates.io publication
- npm wrapper/platform packages
- Homebrew tap update

The shell install script is rendered during release and published with the release assets.

---

## 13. Project Structure

```text
src/
  main.rs
  lib.rs
  cli.rs
  input.rs
  hack.rs
  tld.rs
  cache.rs
  http.rs
  resolve/
    mod.rs
    wire.rs
    slab.rs
    wheel.rs
    socket.rs
    engine.rs
  output.rs
  update.rs
  error.rs
  bin/
    cache_builder.rs

tests/
  cli.rs
  cache.rs
  resolve.rs
  hack.rs
  output.rs
  update.rs
  live_dns_smoke.rs
  common/
```

---

## 14. Testing Strategy

### 14.1 Test categories

| Category | Coverage |
|---|---|
| Input validation | normalization, errors, range parsing |
| Hack detection | suffix matching and ordering |
| Cache | bitmap indices, parsing, download/update behavior |
| Resolver | wire-format parsing, UDP retries, timeout collapse |
| Output | plain text and NDJSON formatting |
| Update | GitHub release check behavior |
| CLI | end-to-end output and exit codes |
| Live smoke | ignored tests against public resolvers |

### 14.2 Mock DNS strategy

Resolver integration tests use a loopback UDP mock DNS server that:

- parses incoming question names
- returns chosen `rcode` and `answer_count`
- can deliberately drop packets to test retry behavior

---

## 15. CI/CD Pipeline

### 15.1 CI workflow

Current CI runs:

1. `cargo clippy --all-targets -- -D warnings`
2. `cargo fmt --check`
3. `cargo test --all-features --lib --bins --test cli --test cache --test resolve --test hack --test output --test update`
4. `cargo test --test live_dns_smoke -- --ignored`
5. `cargo build --release`
6. `cargo llvm-cov --ignore-filename-regex '(^|.*/)bin/cache_builder\.rs$' --fail-under-lines 75`

### 15.2 Release workflow

The release workflow builds release binaries for the configured target matrix, publishes release assets, and runs crate/npm/Homebrew publication steps.

---

## 16. Error Handling

### 16.1 Common scenarios

| Scenario | Exit |
|---|---|
| invalid input | `2` |
| cache bootstrap/download failure | `2` |
| cache checksum mismatch | `2` |
| resolver misconfiguration | `2` |
| no available results | `1` |

### 16.2 Current DNS failure policy

Per-domain live DNS failures do not surface as hard command errors.

Instead:

1. retry until attempts are exhausted
2. collapse unresolved lookups to `unavailable`
3. continue normal output generation

This means the CLI never emits a per-run "N TLDs could not be checked" note in the current implementation.

---

## 17. Performance Notes

- Warm-cache short-domain lookups are effectively instant bitmap reads.
- 4+ character lookups are dominated by resolver RTT and resolver health.
- The resolver avoids per-query HTTP/TLS/JSON overhead.
- Builder throughput is network- and resolver-bound rather than CPU-bound.

No benchmark number is treated as a stable contractual interface in this spec.

---

## 18. Dependencies

### 18.1 Core crates

| Crate | Purpose |
|---|---|
| `clap` | CLI parsing |
| `tokio` | async runtime for main entrypoint and background tasks |
| `reqwest` | cache download, update check, TLD list fetch |
| `serde` / `serde_json` | metadata and JSON parsing |
| `flate2` | cache gzip decompression |
| `sha2` | SHA-256 checksum verification |
| `dirs` | platform cache directory lookup |
| `memmap2` | memory-map local cache file |
| `is-terminal` | TTY detection for color |
| `mio` | readiness polling for live UDP DNS resolver |
| `socket2` | UDP socket creation and socket-option tuning |
| `rand` | transaction IDs and probe names |
| `semver` | update-version comparison |

### 18.2 Dependency split

- HTTP stack: cache/update/TLD source only
- UDP resolver: custom resolver engine in `src/resolve/`
- no DoH client
- no external `massdns` dependency
