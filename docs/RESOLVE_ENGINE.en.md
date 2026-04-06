# Resolve Engine - Design Document

> High-performance bulk DNS resolution engine for domaingrep.
> Replaces both the DoH-based CLI resolver and the massdns-based cache builder
> with a single Rust-native UDP stub resolver designed to match or exceed
> massdns-level throughput (~350K qps).

**Status:** Historical design note. The current implementation shares the Rust UDP resolver across CLI and cache-builder, but does not yet implement every performance optimization described below (notably `sendmmsg`/`recvmmsg`).
**Last Updated:** 2026-04-06

---

## Table of Contents

1. [Goals](#1-goals)
2. [Non-Goals](#2-non-goals)
3. [Architecture Overview](#3-architecture-overview)
4. [Module Structure](#4-module-structure)
5. [DNS Wire Format (`wire.rs`)](#5-dns-wire-format-wirers)
6. [Lookup Slab (`slab.rs`)](#6-lookup-slab-slabrs)
7. [Timing Wheel (`wheel.rs`)](#7-timing-wheel-wheelrs)
8. [Platform I/O Layer (`socket.rs`)](#8-platform-io-layer-socketrs)
9. [Core Engine (`engine.rs`)](#9-core-engine-enginers)
10. [Classification Rule](#10-classification-rule)
11. [Public API (`mod.rs`)](#11-public-api-modrs)
12. [Integration Changes](#12-integration-changes)
13. [Migration Plan](#13-migration-plan)
14. [Testing Strategy](#14-testing-strategy)
15. [Performance Analysis](#15-performance-analysis)
16. [Risks and Mitigations](#16-risks-and-mitigations)

---

## 1. Goals

1. **Match or exceed massdns throughput** -- Target >=300K queries/sec on Linux
   for the cache builder use case (~59M domains).
2. **Single engine** -- CLI (4+ char) and cache-builder share the exact same
   resolution code and classification function.
3. **Single binary** -- No external process dependency. Self-contained Rust binary.
4. **Two-state output** -- Every domain is `available` or `unavailable`. No third state.
5. **NS query type** -- All queries use `QTYPE=NS`.
6. **Shared classification** -- One function (`is_available`) is the single source of truth.

## 2. Non-Goals

- Full recursive DNS resolver.
- DNSSEC validation.
- TCP fallback (NS checks never need large responses).
- Wire-compatible massdns reimplementation.
- Parsing DNS answer/authority/additional records (only RCODE needed).

---

## 3. Architecture Overview

### Why massdns is fast

massdns achieves 200-350K qps through five key design choices:

| Technique | Impact |
|---|---|
| **Pre-allocated lookup slots** (slab) | Zero malloc in hot path |
| **Direct epoll** (no framework) | No runtime/scheduler overhead |
| **Single-threaded event loop** | No locks, no context switches |
| **Timing wheel** for timeouts | O(1) add/remove/expire |
| **Raw UDP sendto/recvfrom** in tight loop | Minimal per-packet overhead |

### Our design: match each technique

| massdns technique | Our equivalent |
|---|---|
| Pre-allocated `lookup_t` array + free list | `Slab<LookupSlot>` with generation counters |
| Custom hashmap keyed by `(name, type)` | Direct-indexed array by transaction ID (65536 slots) |
| `timed_ring` (circular buffer, O(1)) | `TimingWheel` (same algorithm, Rust) |
| `epoll_wait` + `recvfrom` + `sendto` | `mio::Poll` + `sendmmsg`/`recvmmsg` (Linux) |
| Single-threaded C event loop | Single-threaded Rust event loop (no tokio) |

### Critical decision: no async runtime

tokio adds measurable overhead:
- Task scheduling and waker machinery (~50-100ns per `.await`)
- State machine generation for async fns
- Memory overhead per task

For a tight event loop doing hundreds of thousands of UDP sends/recvs per second,
this overhead is significant. We use **mio directly** instead.

### Before vs After

```
Before:
  CLI 4+ char  ──> reqwest + TLS + HTTP/2 + JSON (DoH)     ──> ~10K qps
  Cache builder ──> massdns subprocess (C, raw UDP, epoll)  ──> ~350K qps

After:
  CLI 4+ char  ──┐
                  ├──> src/resolve/ (Rust, raw UDP, mio)    ──> ~300-500K qps
  Cache builder ──┘
```

---

## 4. Module Structure

```
src/resolve/
  mod.rs          Public API: ResolveConfig, resolve_domains(), is_available()
  wire.rs         DNS wire format: encode name, build query, parse response header
  slab.rs         Pre-allocated lookup slab with free list and generation counters
  wheel.rs        Timing wheel for O(1) timeout management
  socket.rs       Platform I/O: sendmmsg/recvmmsg on Linux, fallback on others
  engine.rs       Core event loop: poll -> recv -> process -> send (no async)
```

Estimated size: ~1,200-1,500 lines total.

### Dependencies

| Crate | Purpose | Already in tree? |
|---|---|---|
| `mio` | Event loop (epoll/kqueue/IOCP wrapper) | No (new, ~15KB) |
| `socket2` | Socket creation with options (SO_RCVBUF, etc.) | No (new, ~30KB) |
| `libc` | `sendmmsg`/`recvmmsg` on Linux | Yes |
| `rand` | Transaction ID generation | Yes |

`mio` and `socket2` are lightweight, zero-dependency crates widely used in the Rust ecosystem (tokio depends on mio internally).

---

## 5. DNS Wire Format (`wire.rs`)

Identical to the previous design. This is not a performance bottleneck.

### 5.1 Functions

```rust
pub fn encode_name(name: &str, buf: &mut [u8]) -> Option<usize>
pub fn build_query(buf: &mut [u8], id: u16, name: &str, qtype: u16) -> Option<usize>
pub fn parse_response_header(buf: &[u8]) -> Option<ResponseHeader>

pub struct ResponseHeader {
    pub id: u16,
    pub rcode: u8,
}
```

### 5.2 Key optimization

All functions write into caller-provided buffers. No allocation.
Query packets are built into a stack buffer in the engine's send loop.

---

## 6. Lookup Slab (`slab.rs`)

### 6.1 Why not HashMap

massdns uses two data structures for in-flight tracking:
1. Pre-allocated array of `lookup_t` (the slab)
2. A hashmap keyed by `(name, type)` to find lookups by response content

For domaingrep, we can do better. Since we control transaction IDs and they're
16-bit (0-65535), we use a **direct-indexed array**:

```rust
pub struct LookupSlab {
    slots: Vec<Option<LookupSlot>>,  // size 65536, indexed by transaction ID
    active: usize,
}

pub struct LookupSlot {
    pub domain_index: u32,      // index into input domain list
    pub attempts: u8,           // attempts so far
    pub resolver_index: u16,    // which resolver was used
    pub sent_at: Instant,       // for timeout detection (also used by timing wheel)
    pub wheel_slot: u32,        // position in timing wheel for O(1) removal
}
```

### 6.2 Operations

All O(1):

```rust
impl LookupSlab {
    /// Allocate a slot with a unique transaction ID. O(1) amortized.
    pub fn insert(&mut self, slot: LookupSlot) -> u16

    /// Look up by transaction ID. O(1).
    pub fn get(&self, id: u16) -> Option<&LookupSlot>

    /// Remove by transaction ID. O(1).
    pub fn remove(&mut self, id: u16) -> Option<LookupSlot>

    /// Number of active lookups.
    pub fn active_count(&self) -> usize
}
```

### 6.3 Transaction ID generation

```rust
fn next_id(&self) -> u16 {
    loop {
        let id: u16 = rand::random();
        if self.slots[id as usize].is_none() {
            return id;
        }
    }
}
```

At 10,000 concurrent (15% of 65536), average 1.2 iterations.
At 50,000 concurrent (76% of 65536), average 4.2 iterations.
Concurrency must stay below ~60,000 to keep collision rate acceptable.

### 6.4 Memory

65,536 slots x ~32 bytes = ~2 MB. Fixed, no growth.

---

## 7. Timing Wheel (`wheel.rs`)

### 7.1 Why not a priority queue

A binary heap (standard timeout approach) gives O(log n) insert/remove.
At 300K qps, that's ~300K x log(10000) x 2 (insert + remove) = ~8M comparisons/sec.

massdns uses a **timing wheel** (circular buffer) for O(1) operations.
We replicate the same algorithm.

### 7.2 Design

```rust
pub struct TimingWheel {
    buckets: Vec<Vec<u16>>,    // each bucket holds transaction IDs
    bucket_count: usize,       // number of buckets (e.g., 1024)
    resolution_ms: u64,        // milliseconds per bucket (e.g., 10)
    current_bucket: usize,     // index of next bucket to process
    last_advance: Instant,     // when we last advanced the wheel
}
```

### 7.3 Operations

```rust
impl TimingWheel {
    /// Insert a transaction ID to fire after `delay_ms`. O(1).
    pub fn insert(&mut self, id: u16, delay_ms: u64)

    /// Remove a transaction ID (e.g., when response received). O(1).
    /// Uses swap-remove within the bucket.
    pub fn remove(&mut self, id: u16, wheel_slot: u32)

    /// Advance the wheel and return all expired transaction IDs.
    /// Called each loop iteration. O(expired_count).
    pub fn advance(&mut self) -> impl Iterator<Item = u16>
}
```

### 7.4 Parameters

| Parameter | Value | Rationale |
|---|---|---|
| `bucket_count` | 1024 | Covers 1024 x 10ms = ~10 seconds |
| `resolution_ms` | 10 | 10ms granularity is sufficient for DNS timeouts |

### 7.5 Memory

1024 buckets x ~24 bytes (Vec overhead) + entries.
At 10K concurrent, ~10K x 2 bytes (u16) = ~20 KB. Negligible.

---

## 8. Platform I/O Layer (`socket.rs`)

### 8.1 Overview

This is where the biggest performance difference from the tokio design lives.
Platform-specific batch I/O syscalls reduce system call overhead by 10-50x.

### 8.2 Linux: `sendmmsg` / `recvmmsg`

Batch multiple UDP sends/receives into a single syscall.

```rust
#[cfg(target_os = "linux")]
pub fn send_batch(
    fd: RawFd,
    packets: &[OutPacket],   // pre-built DNS query packets
) -> usize                   // number successfully sent

#[cfg(target_os = "linux")]
pub fn recv_batch(
    fd: RawFd,
    buffers: &mut [InPacket], // pre-allocated receive buffers
) -> usize                   // number received
```

Implementation calls `libc::sendmmsg` / `libc::recvmmsg` directly.

Batch size: 64 packets per syscall (tunable). This means:
- Without batching: 300K sends = 300K `sendto()` syscalls
- With batching (64): 300K sends = ~4,700 `sendmmsg()` syscalls

### 8.3 macOS / other Unix: `sendto` / `recvfrom` loop

No batch syscalls available. Falls back to individual calls, still
non-blocking via mio.

```rust
#[cfg(not(target_os = "linux"))]
pub fn send_batch(fd: RawFd, packets: &[OutPacket]) -> usize {
    // Loop calling sendto() for each packet
}

#[cfg(not(target_os = "linux"))]
pub fn recv_batch(fd: RawFd, buffers: &mut [InPacket]) -> usize {
    // Loop calling recvfrom() until EAGAIN
}
```

### 8.4 Socket creation

```rust
pub fn create_udp_sockets(count: usize) -> Vec<UdpSocket> {
    // Use socket2 for fine-grained control:
    // - SO_RCVBUF: 4 MB (large receive buffer to avoid drops)
    // - SO_SNDBUF: 4 MB (large send buffer)
    // - SO_REUSEPORT: on Linux, for multi-socket load distribution
    // - Non-blocking mode
}
```

### 8.5 Multiple sockets

Multiple UDP sockets avoid kernel-level per-socket lock contention:

| Use case | Socket count | Rationale |
|---|---|---|
| CLI | 1 | ~1,200 queries, no contention |
| Cache builder | 4-8 | High throughput, reduce contention |

Sockets are round-robin assigned to queries.

### 8.6 Pre-allocated I/O buffers

```rust
pub struct IoBuffers {
    send_bufs: Vec<[u8; 512]>,   // pre-allocated send buffers
    recv_bufs: Vec<[u8; 512]>,   // pre-allocated receive buffers
    mmsghdr_send: Vec<libc::mmsghdr>,  // for sendmmsg (Linux)
    mmsghdr_recv: Vec<libc::mmsghdr>,  // for recvmmsg (Linux)
    iovec_send: Vec<libc::iovec>,
    iovec_recv: Vec<libc::iovec>,
}
```

All buffers are allocated once at engine startup. Zero allocation during resolution.

---

## 9. Core Engine (`engine.rs`)

### 9.1 Overview

A single-threaded, synchronous event loop. No async, no tokio, no futures.
Directly mirrors massdns's architecture.

### 9.2 Engine state

```rust
pub struct Engine {
    // Configuration
    config: EngineConfig,
    resolvers: Vec<SocketAddr>,

    // Sockets
    sockets: Vec<mio::net::UdpSocket>,
    poll: mio::Poll,

    // In-flight tracking
    slab: LookupSlab,          // 65536-slot direct-indexed array
    wheel: TimingWheel,        // O(1) timeout management

    // Work queues
    pending: VecDeque<u32>,    // domain indices waiting to be sent
    results: Vec<Option<u8>>,  // final RCODE per domain (None = not yet resolved)

    // I/O buffers (pre-allocated)
    io: IoBuffers,

    // Stats
    completed: usize,
    total: usize,
}
```

### 9.3 Event loop

```rust
pub fn run(config: &EngineConfig, domains: &[String]) -> Vec<Option<u8>> {
    let mut engine = Engine::new(config, domains);

    loop {
        // 1. Fill send queue: move domains from pending -> in-flight
        engine.fill_send_queue();

        // 2. Batch send: sendmmsg on Linux, sendto loop on others
        engine.flush_sends();

        // 3. Poll for readability (mio)
        engine.poll_events();

        // 4. Batch receive: recvmmsg on Linux, recvfrom loop on others
        engine.drain_receives();

        // 5. Process received responses
        //    - Definitive (NOERROR/NXDOMAIN): record result, free slot
        //    - Non-definitive: re-enqueue if attempts remain, else record failure
        engine.process_responses();

        // 6. Advance timing wheel, handle timeouts
        //    - Re-enqueue if attempts remain
        //    - Record failure if max attempts reached
        engine.handle_timeouts();

        // 7. Check termination
        if engine.completed >= engine.total {
            break;
        }
    }

    engine.results
}
```

### 9.4 fill_send_queue detail

```
While slab.active_count() < config.concurrency AND pending is not empty:
  1. Pop domain_index from pending
  2. Allocate slot in slab -> get transaction ID
  3. Build DNS query packet into pre-allocated send buffer
  4. Pick resolver: resolvers[rand() % resolvers.len()]
  5. Enqueue (packet, resolver_addr) into send batch
  6. Insert transaction ID into timing wheel
```

### 9.5 drain_receives detail

```
Call recv_batch() (recvmmsg on Linux):
  For each received packet:
    1. Parse response header -> (id, rcode)
    2. Look up slab[id]
    3. If slot empty: discard (stale/mismatch)
    4. Remove from timing wheel
    5. If RCODE is 0 (NOERROR) or 3 (NXDOMAIN):
       - Record result: results[domain_index] = Some(rcode)
       - Free slab slot
       - completed++
    6. Else (SERVFAIL, REFUSED, etc.):
       - If attempts < max_attempts:
         - Increment attempts
         - Push domain_index back to pending
         - Free slab slot
       - Else:
         - Record result: results[domain_index] = None (terminal failure)
         - Free slab slot
         - completed++
```

### 9.6 handle_timeouts detail

```
Call wheel.advance():
  For each expired transaction ID:
    1. Look up slab[id]
    2. If slot empty: skip (already resolved)
    3. If attempts < max_attempts:
       - Increment attempts
       - Push domain_index back to pending
       - Free slab slot
    4. Else:
       - Record result: results[domain_index] = None
       - Free slab slot
       - completed++
```

### 9.7 Poll configuration

```rust
// mio poll with 1ms timeout
// - Short timeout ensures timing wheel advances frequently
// - Keeps send queue responsive
poll.poll(&mut events, Some(Duration::from_millis(1)))?;
```

### 9.8 Concurrency control

Default concurrency and limits:

| Use case | Concurrency | Max (limited by u16 ID space) |
|---|---|---|
| CLI | 1,000 | 60,000 |
| Cache builder | 10,000 | 60,000 |

Concurrency is the maximum number of in-flight queries (slab active count).

---

## 10. Classification Rule

Unchanged from previous design. One function, used everywhere:

```rust
pub fn is_available(rcode: Option<u8>) -> bool {
    rcode == Some(RCODE_NXDOMAIN)
}
```

---

## 11. Public API (`mod.rs`)

### 11.1 Configuration

```rust
pub struct ResolveConfig {
    pub resolvers: Vec<SocketAddr>,
    pub concurrency: usize,
    pub query_timeout_ms: u64,
    pub max_attempts: u8,
    pub socket_count: usize,
    pub send_batch_size: usize,  // packets per sendmmsg call
    pub recv_batch_size: usize,  // packets per recvmmsg call
    pub recv_buf_size: usize,    // SO_RCVBUF
    pub send_buf_size: usize,    // SO_SNDBUF
}
```

### 11.2 Batch resolution

```rust
/// Resolve availability for a batch of FQDNs.
/// Runs the engine synchronously (blocking the calling thread).
/// Returns one bool per input domain in the same order.
pub fn resolve_domains(
    config: &ResolveConfig,
    domains: &[String],
) -> Result<Vec<bool>, AppError>

/// Raw variant returning Option<u8> RCODEs (for TLD probing).
pub fn resolve_domains_raw(
    config: &ResolveConfig,
    domains: &[String],
) -> Result<Vec<Option<u8>>, AppError>
```

Since the engine is synchronous, callers from async contexts use
`tokio::task::spawn_blocking()` if needed.

---

## 12. Integration Changes

Same as previous design (see previous version). Summary:

| File | Change |
|---|---|
| `src/dns.rs` | **Delete** |
| `src/resolve/*` | **Add** (6 files) |
| `src/http.rs` | **Add** (extracted `build_http_client` from dns.rs) |
| `src/lib.rs` | Replace DnsResolver with resolve module |
| `src/bin/cache_builder.rs` | Remove inline massdns, use resolve module |
| `src/tld.rs` | Batch probing via resolve module |
| `Cargo.toml` | Add `mio`, `socket2`; remove `futures` |

---

## 13. Migration Plan

Same phased approach as previous design:

1. **Phase 1:** Add resolve module (non-breaking)
2. **Phase 2:** Switch CLI to resolve module
3. **Phase 3:** Switch cache-builder to resolve module
4. **Phase 4:** Switch TLD probing
5. **Phase 5:** Cleanup

---

## 14. Testing Strategy

Same categories as previous design, plus performance benchmarks:

### 14.1 Unit tests (no network)

- Wire format encoding/decoding
- Slab insert/remove/lookup
- Timing wheel insert/advance/expire
- `is_available()` truth table

### 14.2 Integration tests (loopback UDP)

- Mock DNS server on localhost
- Correctness of classification
- Retry logic
- Timeout handling

### 14.3 Performance benchmarks

```rust
#[bench]
fn bench_resolve_10k_domains() {
    // Resolve 10,000 domains against a mock server
    // Measure: queries per second, p50/p99 latency
}

#[bench]
fn bench_wire_build_query() {
    // Build 1M query packets
    // Measure: packets per second
}

#[bench]
fn bench_slab_insert_remove() {
    // 1M insert/remove cycles
    // Measure: operations per second
}
```

---

## 15. Performance Analysis

### 15.1 Per-query cost breakdown

| Operation | massdns (C) | Our design (Rust) | Notes |
|---|---|---|---|
| Build query packet | ~50ns | ~50ns | Same byte operations |
| Send (sendto) | ~200ns | ~200ns | Same syscall |
| Send (sendmmsg, batch 64) | N/A | ~5ns amortized | 64x fewer syscalls |
| Receive (recvfrom) | ~200ns | ~200ns | Same syscall |
| Receive (recvmmsg, batch 64) | N/A | ~5ns amortized | 64x fewer syscalls |
| Parse response | ~30ns | ~30ns | Same 4-byte read |
| Slab lookup by ID | ~80ns (hashmap) | ~3ns (array index) | Direct indexing wins |
| Timing wheel insert | ~20ns | ~20ns | Same algorithm |
| Timing wheel remove | ~5ns (pointer nil) | ~20ns (swap-remove) | Slightly slower |
| Event loop overhead | ~0ns (raw epoll) | ~20ns (mio wraps epoll) | Minimal |
| **Total per query** | **~585ns** | **~353ns** | **~40% faster** |

### 15.2 System call reduction (Linux)

| Operation | Without batching | With sendmmsg/recvmmsg (batch 64) |
|---|---|---|
| 300K sends/sec | 300K sendto() calls | ~4,700 sendmmsg() calls |
| 300K recvs/sec | 300K recvfrom() calls | ~4,700 recvmmsg() calls |
| Total syscalls/sec | ~600K | ~9,400 |
| **Reduction** | | **98.4%** |

### 15.3 Expected throughput

| Platform | Technique | Expected qps |
|---|---|---|
| Linux (cache builder) | mio + sendmmsg/recvmmsg + multi-socket | 300-500K |
| Linux (CLI) | mio + sendmmsg/recvmmsg | 300-500K (but only ~1,200 queries) |
| macOS (CLI) | mio + sendto/recvfrom | 100-200K |
| Windows (CLI) | mio + sendto/recvfrom | 50-150K |

### 15.4 Why we can potentially beat massdns

1. **sendmmsg/recvmmsg**: massdns uses individual `sendto`/`recvfrom` calls.
   Batching reduces syscall overhead by ~98%.
2. **Direct-indexed slab**: massdns uses a hashmap for lookup matching.
   Array indexing by transaction ID is ~25x faster per lookup.
3. **Multiple sockets**: massdns defaults to 1 socket.
   Multiple sockets reduce kernel lock contention.

### 15.5 Why we might not beat massdns

1. **Rust bounds checking**: Every array access includes a bounds check.
   Mitigated by using `get_unchecked` in hot paths (with safety proof).
2. **mio overhead**: ~20ns per poll iteration vs raw epoll.
3. **Memory layout**: massdns's C structs have zero padding control.
   Rust's `#[repr(C)]` can match this if needed.

### 15.6 Bottleneck analysis

At 300K+ qps, the real bottleneck is none of the above. It's:
1. **Network RTT**: 10-50ms per query means we need 3K-15K concurrent to saturate.
2. **Resolver capacity**: Public resolvers may rate-limit or slow down.
3. **Kernel UDP stack**: At very high rates, socket buffer overflows cause drops.

Our design addresses (3) with large SO_RCVBUF/SO_SNDBUF and multiple sockets.
(1) and (2) are controlled by concurrency and resolver list configuration.

---

## 16. Risks and Mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| sendmmsg/recvmmsg Linux-only | Medium | Graceful fallback to individual calls on other platforms |
| 65536 transaction ID space | Low | Limits concurrent to ~60K; sufficient for all use cases |
| Stale response misattribution | Low | Transaction ID checked on every response; slots freed promptly |
| `unsafe` code for hot-path perf | Medium | Minimal unsafe blocks with documented safety invariants; fuzz tested |
| mio API changes | Low | mio is stable (0.8+); pin version |
| Higher code complexity | Accepted | Trade-off for performance as stated in requirements |

---

## Appendix A: massdns Architecture Comparison

### What massdns does (7,132 lines C)

| Component | Lines | Our approach | Our lines (est.) |
|---|---|---|---|
| DNS wire format (full) | ~1,750 | Minimal subset | ~100 |
| Hashmap (in-flight) | ~340 | Direct-indexed array | ~80 |
| Timed ring (timeouts) | ~155 | Timing wheel | ~120 |
| Event loop (epoll) | ~500 | mio + sendmmsg/recvmmsg | ~300 |
| Retry / resolver rotation | ~100 | Same logic | ~60 |
| Socket setup / buffers | ~200 | socket2 + batch I/O setup | ~150 |
| Multi-process fork | ~300 | Not needed | 0 |
| TCP support | ~400 | Not needed | 0 |
| Raw socket / IPv6 src | ~200 | Not needed | 0 |
| Output formats | ~500 | Not needed (internal API) | 0 |
| CLI / stats / privilege | ~800 | Not needed | 0 |
| Public API + classification | 0 | New | ~80 |
| Platform abstraction | 0 | New (sendmmsg/fallback) | ~200 |
| **Total** | **~5,245** (relevant) | | **~1,090** |

### What we add beyond massdns (conceptually)

1. **sendmmsg/recvmmsg batch I/O** -- massdns doesn't use this
2. **Direct-indexed array** instead of hashmap -- faster lookup
3. **Multiple sockets** with round-robin -- massdns defaults to 1
4. **Cross-platform** via mio abstraction -- massdns is Linux/macOS only
