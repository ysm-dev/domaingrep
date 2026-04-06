# Resolve Engine - 설계 문서

> domaingrep 고성능 대량 DNS 조회 엔진.
> DoH 기반 CLI 리졸버와 massdns 기반 캐시 빌더를 모두 대체하여,
> massdns급 처리량(~350K qps)에 도달하거나 초과하는 것을 목표로 하는
> 단일 Rust 네이티브 UDP stub resolver.

**상태:** 과거 설계 노트. 현재 구현은 Rust UDP resolver를 CLI와 cache-builder에 공용으로 사용하지만, 아래의 모든 성능 최적화(특히 `sendmmsg`/`recvmmsg`)를 아직 구현한 것은 아니다.
**최종 수정:** 2026-04-06

---

## 목차

1. [목표](#1-목표)
2. [비목표](#2-비목표)
3. [아키텍처 개요](#3-아키텍처-개요)
4. [모듈 구조](#4-모듈-구조)
5. [DNS Wire Format (`wire.rs`)](#5-dns-wire-format-wirers)
6. [Lookup Slab (`slab.rs`)](#6-lookup-slab-slabrs)
7. [Timing Wheel (`wheel.rs`)](#7-timing-wheel-wheelrs)
8. [플랫폼 I/O 레이어 (`socket.rs`)](#8-플랫폼-io-레이어-socketrs)
9. [코어 엔진 (`engine.rs`)](#9-코어-엔진-enginers)
10. [판정 규칙](#10-판정-규칙)
11. [공개 API (`mod.rs`)](#11-공개-api-modrs)
12. [통합 변경 사항](#12-통합-변경-사항)
13. [마이그레이션 계획](#13-마이그레이션-계획)
14. [테스트 전략](#14-테스트-전략)
15. [성능 분석](#15-성능-분석)
16. [리스크와 대응](#16-리스크와-대응)

---

## 1. 목표

1. **massdns 처리량에 도달하거나 초과** -- 캐시 빌더 사용 사례(~59M 도메인)에서
   Linux 기준 >=300K qps 목표.
2. **단일 엔진** -- CLI (4글자 이상)와 cache-builder가 정확히 같은 조회 코드와
   판정 함수를 사용.
3. **단일 바이너리** -- 외부 프로세스 의존성 없음. 자급자족 Rust 바이너리.
4. **2-state 출력** -- 모든 도메인은 `available` 또는 `unavailable`. 제3의 상태 없음.
5. **NS 질의 타입** -- 모든 질의에 `QTYPE=NS` 사용.
6. **공유 판정 함수** -- `is_available()` 하나가 유일한 판정 기준.

## 2. 비목표

- 전체 재귀 DNS 리졸버 구현.
- DNSSEC 검증.
- TCP 폴백 (NS 체크는 큰 응답이 필요하지 않음).
- massdns의 전체 기능을 wire-compatible하게 재구현.
- DNS 응답의 answer/authority/additional 레코드 파싱 (RCODE만 필요).

---

## 3. 아키텍처 개요

### massdns가 빠른 이유

massdns가 200-350K qps를 달성하는 다섯 가지 핵심 설계 선택:

| 기법 | 영향 |
|---|---|
| **사전 할당된 lookup 슬롯** (slab) | hot path에서 malloc 제로 |
| **직접 epoll** (프레임워크 없음) | 런타임/스케줄러 오버헤드 없음 |
| **단일 스레드 이벤트 루프** | 락 없음, 컨텍스트 스위치 없음 |
| **Timing wheel**로 타임아웃 관리 | O(1) 추가/제거/만료 |
| **Raw UDP sendto/recvfrom** tight loop | 패킷당 오버헤드 최소 |

### 우리 설계: 각 기법을 매칭

| massdns 기법 | 우리의 동등물 |
|---|---|
| 사전 할당 `lookup_t` 배열 + free list | `Slab<LookupSlot>` + 세대 카운터 |
| `(name, type)` 키 커스텀 해시맵 | 트랜잭션 ID로 직접 인덱싱하는 배열 (65536 슬롯) |
| `timed_ring` (순환 버퍼, O(1)) | `TimingWheel` (동일 알고리즘, Rust) |
| `epoll_wait` + `recvfrom` + `sendto` | `mio::Poll` + `sendmmsg`/`recvmmsg` (Linux) |
| 단일 스레드 C 이벤트 루프 | 단일 스레드 Rust 이벤트 루프 (tokio 없음) |

### 핵심 결정: async 런타임 미사용

tokio는 측정 가능한 오버헤드를 추가한다:
- 태스크 스케줄링 및 waker 기계 (~50-100ns per `.await`)
- async fn에 대한 상태 머신 생성
- 태스크당 메모리 오버헤드

초당 수십만 건의 UDP 송수신을 하는 tight loop에서 이 오버헤드는 유의미하다.
대신 **mio를 직접** 사용한다.

### 변경 전 vs 후

```
변경 전:
  CLI 4+ 글자  ──> reqwest + TLS + HTTP/2 + JSON (DoH)     ──> ~10K qps
  캐시 빌더   ──> massdns 서브프로세스 (C, raw UDP, epoll)  ──> ~350K qps

변경 후:
  CLI 4+ 글자  ──┐
                  ├──> src/resolve/ (Rust, raw UDP, mio)    ──> ~300-500K qps
  캐시 빌더   ──┘
```

---

## 4. 모듈 구조

```
src/resolve/
  mod.rs          공개 API: ResolveConfig, resolve_domains(), is_available()
  wire.rs         DNS wire format: 이름 인코딩, 질의 빌드, 응답 헤더 파싱
  slab.rs         사전 할당 lookup slab + free list + 세대 카운터
  wheel.rs        O(1) 타임아웃 관리용 timing wheel
  socket.rs       플랫폼 I/O: Linux에서 sendmmsg/recvmmsg, 기타에서 폴백
  engine.rs       코어 이벤트 루프: poll -> recv -> process -> send (async 없음)
```

예상 규모: 총 ~1,200-1,500줄.

### 의존성

| 크레이트 | 목적 | 이미 트리에 있는가? |
|---|---|---|
| `mio` | 이벤트 루프 (epoll/kqueue/IOCP 래퍼) | 아니오 (신규, ~15KB) |
| `socket2` | 소켓 옵션 설정 (SO_RCVBUF 등) | 아니오 (신규, ~30KB) |
| `libc` | Linux에서 `sendmmsg`/`recvmmsg` | 예 |
| `rand` | 트랜잭션 ID 생성 | 예 |

`mio`와 `socket2`는 Rust 생태계에서 널리 사용되는 경량 제로 의존성 크레이트이다
(tokio가 내부적으로 mio에 의존).

---

## 5. DNS Wire Format (`wire.rs`)

이전 설계와 동일. 성능 병목이 아님.

### 5.1 함수

```rust
pub fn encode_name(name: &str, buf: &mut [u8]) -> Option<usize>
pub fn build_query(buf: &mut [u8], id: u16, name: &str, qtype: u16) -> Option<usize>
pub fn parse_response_header(buf: &[u8]) -> Option<ResponseHeader>

pub struct ResponseHeader {
    pub id: u16,
    pub rcode: u8,
}
```

### 5.2 핵심 최적화

모든 함수가 호출자 제공 버퍼에 기록. 할당 없음.
질의 패킷은 엔진 송신 루프의 스택 버퍼에 직접 구성.

---

## 6. Lookup Slab (`slab.rs`)

### 6.1 왜 HashMap이 아닌가

트랜잭션 ID가 16비트 (0-65535)이므로, **직접 인덱싱 배열**을 사용:

```rust
pub struct LookupSlab {
    slots: Vec<Option<LookupSlot>>,  // 크기 65536, 트랜잭션 ID로 인덱싱
    active: usize,
}

pub struct LookupSlot {
    pub domain_index: u32,      // 입력 도메인 목록의 인덱스
    pub attempts: u8,           // 지금까지 시도 횟수
    pub resolver_index: u16,    // 사용한 리졸버
    pub sent_at: Instant,       // 타임아웃 감지용
    pub wheel_slot: u32,        // timing wheel에서의 위치 (O(1) 제거용)
}
```

### 6.2 연산

모두 O(1):

```rust
impl LookupSlab {
    /// 고유한 트랜잭션 ID로 슬롯 할당. O(1) amortized.
    pub fn insert(&mut self, slot: LookupSlot) -> u16

    /// 트랜잭션 ID로 조회. O(1).
    pub fn get(&self, id: u16) -> Option<&LookupSlot>

    /// 트랜잭션 ID로 제거. O(1).
    pub fn remove(&mut self, id: u16) -> Option<LookupSlot>

    /// 활성 조회 수.
    pub fn active_count(&self) -> usize
}
```

### 6.3 메모리

65,536 슬롯 x ~32바이트 = ~2 MB. 고정, 증가 없음.

---

## 7. Timing Wheel (`wheel.rs`)

### 7.1 왜 우선순위 큐가 아닌가

이진 힙 (표준 타임아웃 접근)은 O(log n) 삽입/제거.
300K qps에서, ~300K x log(10000) x 2 = ~8M 비교/초.

massdns는 O(1) 연산을 위해 **timing wheel** (순환 버퍼)을 사용.
같은 알고리즘을 복제한다.

### 7.2 설계

```rust
pub struct TimingWheel {
    buckets: Vec<Vec<u16>>,    // 각 버킷은 트랜잭션 ID를 보유
    bucket_count: usize,       // 버킷 수 (예: 1024)
    resolution_ms: u64,        // 버킷당 밀리초 (예: 10)
    current_bucket: usize,     // 다음 처리할 버킷의 인덱스
    last_advance: Instant,     // 마지막으로 wheel을 전진시킨 시각
}
```

### 7.3 연산

```rust
impl TimingWheel {
    /// delay_ms 후에 발화할 트랜잭션 ID 삽입. O(1).
    pub fn insert(&mut self, id: u16, delay_ms: u64)

    /// 트랜잭션 ID 제거 (예: 응답 수신 시). O(1).
    pub fn remove(&mut self, id: u16, wheel_slot: u32)

    /// Wheel을 전진시키고 만료된 모든 트랜잭션 ID를 반환.
    pub fn advance(&mut self) -> impl Iterator<Item = u16>
}
```

### 7.4 파라미터

| 파라미터 | 값 | 근거 |
|---|---|---|
| `bucket_count` | 1024 | 1024 x 10ms = ~10초 커버 |
| `resolution_ms` | 10 | DNS 타임아웃에 10ms 정밀도면 충분 |

---

## 8. 플랫폼 I/O 레이어 (`socket.rs`)

### 8.1 개요

tokio 설계와의 가장 큰 성능 차이가 여기에 있다.
플랫폼별 배치 I/O 시스콜이 시스템 콜 오버헤드를 10-50배 줄인다.

### 8.2 Linux: `sendmmsg` / `recvmmsg`

여러 UDP 송수신을 하나의 시스콜로 배치 처리.

```rust
#[cfg(target_os = "linux")]
pub fn send_batch(fd: RawFd, packets: &[OutPacket]) -> usize

#[cfg(target_os = "linux")]
pub fn recv_batch(fd: RawFd, buffers: &mut [InPacket]) -> usize
```

`libc::sendmmsg` / `libc::recvmmsg`를 직접 호출.

배치 크기: 시스콜당 64개 패킷 (조절 가능). 즉:
- 배치 없이: 300K 전송 = 300K `sendto()` 시스콜
- 배치(64): 300K 전송 = ~4,700 `sendmmsg()` 시스콜

### 8.3 macOS / 기타 Unix: `sendto` / `recvfrom` 루프

배치 시스콜 없음. mio를 통한 non-blocking 개별 호출로 폴백.

### 8.4 소켓 생성

```rust
pub fn create_udp_sockets(count: usize) -> Vec<UdpSocket> {
    // socket2로 세밀한 제어:
    // - SO_RCVBUF: 4 MB (드롭 방지를 위한 큰 수신 버퍼)
    // - SO_SNDBUF: 4 MB (큰 송신 버퍼)
    // - SO_REUSEPORT: Linux에서 다중 소켓 부하 분산
    // - Non-blocking 모드
}
```

### 8.5 다중 소켓

여러 UDP 소켓이 커널 수준의 소켓별 락 경합을 줄인다:

| 사용 사례 | 소켓 수 | 근거 |
|---|---|---|
| CLI | 1 | ~1,200 질의, 경합 없음 |
| 캐시 빌더 | 4-8 | 높은 처리량, 경합 감소 |

### 8.6 사전 할당 I/O 버퍼

```rust
pub struct IoBuffers {
    send_bufs: Vec<[u8; 512]>,   // 사전 할당된 송신 버퍼
    recv_bufs: Vec<[u8; 512]>,   // 사전 할당된 수신 버퍼
    mmsghdr_send: Vec<libc::mmsghdr>,  // sendmmsg용 (Linux)
    mmsghdr_recv: Vec<libc::mmsghdr>,  // recvmmsg용 (Linux)
    iovec_send: Vec<libc::iovec>,
    iovec_recv: Vec<libc::iovec>,
}
```

모든 버퍼는 엔진 시작 시 한 번 할당. 조회 중 할당 제로.

---

## 9. 코어 엔진 (`engine.rs`)

### 9.1 개요

단일 스레드, 동기 이벤트 루프. async 없음, tokio 없음, futures 없음.
massdns의 아키텍처를 직접 반영.

### 9.2 엔진 상태

```rust
pub struct Engine {
    // 설정
    config: EngineConfig,
    resolvers: Vec<SocketAddr>,

    // 소켓
    sockets: Vec<mio::net::UdpSocket>,
    poll: mio::Poll,

    // In-flight 추적
    slab: LookupSlab,          // 65536 슬롯 직접 인덱싱 배열
    wheel: TimingWheel,        // O(1) 타임아웃 관리

    // 작업 큐
    pending: VecDeque<u32>,    // 전송 대기 중인 도메인 인덱스
    results: Vec<Option<u8>>,  // 도메인별 최종 RCODE (None = 아직 미해결)

    // I/O 버퍼 (사전 할당)
    io: IoBuffers,

    // 통계
    completed: usize,
    total: usize,
}
```

### 9.3 이벤트 루프

```rust
pub fn run(config: &EngineConfig, domains: &[String]) -> Vec<Option<u8>> {
    let mut engine = Engine::new(config, domains);

    loop {
        // 1. 송신 큐 채우기: pending -> in-flight로 도메인 이동
        engine.fill_send_queue();

        // 2. 배치 전송: Linux에서 sendmmsg, 기타에서 sendto 루프
        engine.flush_sends();

        // 3. 읽기 가능 이벤트 폴링 (mio)
        engine.poll_events();

        // 4. 배치 수신: Linux에서 recvmmsg, 기타에서 recvfrom 루프
        engine.drain_receives();

        // 5. 수신된 응답 처리
        //    - 확정적 (NOERROR/NXDOMAIN): 결과 기록, 슬롯 해제
        //    - 비확정적: 시도 남으면 재큐잉, 아니면 실패 기록
        engine.process_responses();

        // 6. Timing wheel 전진, 타임아웃 처리
        //    - 시도 남으면 재큐잉
        //    - 최대 시도 도달 시 실패 기록
        engine.handle_timeouts();

        // 7. 종료 확인
        if engine.completed >= engine.total {
            break;
        }
    }

    engine.results
}
```

### 9.4 fill_send_queue 상세

```
slab.active_count() < config.concurrency 이고 pending이 비어있지 않은 동안:
  1. pending에서 domain_index를 팝
  2. slab에 슬롯 할당 -> 트랜잭션 ID 획득
  3. 사전 할당된 송신 버퍼에 DNS 질의 패킷 빌드
  4. 리졸버 선택: resolvers[rand() % resolvers.len()]
  5. (패킷, resolver_addr)를 송신 배치에 큐잉
  6. Timing wheel에 트랜잭션 ID 삽입
```

### 9.5 drain_receives 상세

```
recv_batch() 호출 (Linux에서 recvmmsg):
  수신된 각 패킷에 대해:
    1. 응답 헤더 파싱 -> (id, rcode)
    2. slab[id] 조회
    3. 슬롯 비어있으면: 폐기 (stale/mismatch)
    4. Timing wheel에서 제거
    5. RCODE가 0 (NOERROR) 또는 3 (NXDOMAIN)이면:
       - 결과 기록: results[domain_index] = Some(rcode)
       - slab 슬롯 해제
       - completed++
    6. 아니면 (SERVFAIL, REFUSED 등):
       - attempts < max_attempts이면:
         - attempts 증가
         - domain_index를 pending에 다시 푸시
         - slab 슬롯 해제
       - 아니면:
         - 결과 기록: results[domain_index] = None (terminal failure)
         - slab 슬롯 해제
         - completed++
```

### 9.6 handle_timeouts 상세

```
wheel.advance() 호출:
  만료된 각 트랜잭션 ID에 대해:
    1. slab[id] 조회
    2. 슬롯 비어있으면: 건너뜀 (이미 해결됨)
    3. attempts < max_attempts이면:
       - attempts 증가
       - domain_index를 pending에 다시 푸시
       - slab 슬롯 해제
    4. 아니면:
       - 결과 기록: results[domain_index] = None
       - slab 슬롯 해제
       - completed++
```

### 9.7 동시성 제어

| 사용 사례 | 동시성 | 최대 (u16 ID 공간 제한) |
|---|---|---|
| CLI | 1,000 | 60,000 |
| 캐시 빌더 | 10,000 | 60,000 |

---

## 10. 판정 규칙

이전 설계와 동일. 하나의 함수, 모든 곳에서 사용:

```rust
pub fn is_available(rcode: Option<u8>) -> bool {
    rcode == Some(RCODE_NXDOMAIN)
}
```

---

## 11. 공개 API (`mod.rs`)

### 11.1 설정

```rust
pub struct ResolveConfig {
    pub resolvers: Vec<SocketAddr>,
    pub concurrency: usize,
    pub query_timeout_ms: u64,
    pub max_attempts: u8,
    pub socket_count: usize,
    pub send_batch_size: usize,  // sendmmsg 호출당 패킷 수
    pub recv_batch_size: usize,  // recvmmsg 호출당 패킷 수
    pub recv_buf_size: usize,    // SO_RCVBUF
    pub send_buf_size: usize,    // SO_SNDBUF
}
```

### 11.2 배치 조회

```rust
/// FQDN 배치의 가용성을 조회.
/// 엔진을 동기적으로 실행 (호출 스레드를 블록).
/// 입력 도메인당 하나의 bool을 같은 순서로 반환.
pub fn resolve_domains(
    config: &ResolveConfig,
    domains: &[String],
) -> Result<Vec<bool>, AppError>

/// Raw 변형 -- Option<u8> RCODE를 반환 (TLD 프로빙용).
pub fn resolve_domains_raw(
    config: &ResolveConfig,
    domains: &[String],
) -> Result<Vec<Option<u8>>, AppError>
```

엔진이 동기적이므로, async 컨텍스트의 호출자는
필요시 `tokio::task::spawn_blocking()`을 사용.

---

## 12. 통합 변경 사항

이전 설계와 동일. 요약:

| 파일 | 변경 |
|---|---|
| `src/dns.rs` | **삭제** |
| `src/resolve/*` | **추가** (6 파일) |
| `src/http.rs` | **추가** (dns.rs에서 `build_http_client` 추출) |
| `src/lib.rs` | DnsResolver를 resolve 모듈로 교체 |
| `src/bin/cache_builder.rs` | 인라인 massdns 제거, resolve 모듈 사용 |
| `src/tld.rs` | resolve 모듈을 통한 배치 프로빙 |
| `Cargo.toml` | `mio`, `socket2` 추가; `futures` 제거 |

---

## 13. 마이그레이션 계획

이전 설계와 동일한 단계별 접근:

1. **Phase 1:** resolve 모듈 추가 (비파괴적)
2. **Phase 2:** CLI를 resolve 모듈로 전환
3. **Phase 3:** cache-builder를 resolve 모듈로 전환
4. **Phase 4:** TLD 프로빙 전환
5. **Phase 5:** 정리

---

## 14. 테스트 전략

이전 설계와 동일한 범주에 성능 벤치마크 추가:

### 14.1 유닛 테스트 (네트워크 불필요)

- Wire format 인코딩/디코딩
- Slab 삽입/제거/조회
- Timing wheel 삽입/전진/만료
- `is_available()` 진리표

### 14.2 통합 테스트 (루프백 UDP)

- localhost에서 mock DNS 서버
- 판정의 정확성
- 재시도 로직
- 타임아웃 처리

### 14.3 성능 벤치마크

```rust
#[bench]
fn bench_resolve_10k_domains() {
    // mock 서버에 대해 10,000개 도메인 조회
    // 측정: 초당 질의 수, p50/p99 레이턴시
}

#[bench]
fn bench_wire_build_query() {
    // 1M 질의 패킷 빌드
    // 측정: 초당 패킷 수
}

#[bench]
fn bench_slab_insert_remove() {
    // 1M 삽입/제거 사이클
    // 측정: 초당 연산 수
}
```

---

## 15. 성능 분석

### 15.1 질의당 비용 분석

| 연산 | massdns (C) | 우리 설계 (Rust) | 비고 |
|---|---|---|---|
| 질의 패킷 빌드 | ~50ns | ~50ns | 동일한 바이트 연산 |
| 전송 (sendto) | ~200ns | ~200ns | 동일한 시스콜 |
| 전송 (sendmmsg, 배치 64) | N/A | ~5ns amortized | 시스콜 64배 감소 |
| 수신 (recvfrom) | ~200ns | ~200ns | 동일한 시스콜 |
| 수신 (recvmmsg, 배치 64) | N/A | ~5ns amortized | 시스콜 64배 감소 |
| 응답 파싱 | ~30ns | ~30ns | 동일한 4바이트 읽기 |
| ID로 slab 조회 | ~80ns (hashmap) | ~3ns (배열 인덱스) | 직접 인덱싱 우위 |
| Timing wheel 삽입 | ~20ns | ~20ns | 동일한 알고리즘 |
| Timing wheel 제거 | ~5ns (포인터 nil) | ~20ns (swap-remove) | 약간 느림 |
| 이벤트 루프 오버헤드 | ~0ns (raw epoll) | ~20ns (mio wraps epoll) | 최소 |
| **질의당 합계** | **~585ns** | **~353ns** | **~40% 빠름** |

### 15.2 시스템 콜 감소 (Linux)

| 연산 | 배치 없이 | sendmmsg/recvmmsg (배치 64) |
|---|---|---|
| 300K 전송/초 | 300K sendto() 호출 | ~4,700 sendmmsg() 호출 |
| 300K 수신/초 | 300K recvfrom() 호출 | ~4,700 recvmmsg() 호출 |
| 총 시스콜/초 | ~600K | ~9,400 |
| **감소율** | | **98.4%** |

### 15.3 예상 처리량

| 플랫폼 | 기법 | 예상 qps |
|---|---|---|
| Linux (캐시 빌더) | mio + sendmmsg/recvmmsg + 다중 소켓 | 300-500K |
| Linux (CLI) | mio + sendmmsg/recvmmsg | 300-500K (단 ~1,200 질의) |
| macOS (CLI) | mio + sendto/recvfrom | 100-200K |
| Windows (CLI) | mio + sendto/recvfrom | 50-150K |

### 15.4 massdns를 잠재적으로 초과할 수 있는 이유

1. **sendmmsg/recvmmsg**: massdns는 개별 `sendto`/`recvfrom` 호출을 사용.
   배치 처리가 시스콜 오버헤드를 ~98% 줄임.
2. **직접 인덱싱 slab**: massdns는 조회 매칭에 해시맵 사용.
   트랜잭션 ID 배열 인덱싱이 조회당 ~25배 빠름.
3. **다중 소켓**: massdns는 기본 1소켓.
   다중 소켓이 커널 락 경합을 줄임.

### 15.5 massdns를 초과하지 못할 수 있는 이유

1. **Rust 바운드 체크**: 모든 배열 접근에 바운드 체크 포함.
   핫 패스에서 `get_unchecked` 사용으로 완화 가능 (안전성 증명 동반).
2. **mio 오버헤드**: raw epoll 대비 poll 반복당 ~20ns.
3. **메모리 레이아웃**: massdns의 C 구조체는 패딩 제어가 자유로움.
   필요 시 Rust의 `#[repr(C)]`로 매칭 가능.

### 15.6 병목 분석

300K+ qps에서 실제 병목은 위의 어느 것도 아니다:
1. **네트워크 RTT**: 질의당 10-50ms는 포화시키려면 3K-15K 동시성이 필요
2. **리졸버 용량**: 공개 리졸버가 rate-limit하거나 느려질 수 있음
3. **커널 UDP 스택**: 매우 높은 속도에서 소켓 버퍼 오버플로가 드롭 유발

(3)은 큰 SO_RCVBUF/SO_SNDBUF와 다중 소켓으로 대응.
(1)과 (2)는 동시성과 리졸버 목록 설정으로 제어.

---

## 16. 리스크와 대응

| 리스크 | 심각도 | 대응 |
|---|---|---|
| sendmmsg/recvmmsg Linux 전용 | 중간 | 다른 플랫폼에서 개별 호출로 우아한 폴백 |
| 65536 트랜잭션 ID 공간 | 낮음 | 동시성 ~60K로 제한; 모든 사용 사례에 충분 |
| Stale 응답 오귀속 | 낮음 | 모든 응답에서 트랜잭션 ID 확인; 슬롯 즉시 해제 |
| 핫 패스 성능을 위한 `unsafe` 코드 | 중간 | 최소한의 unsafe 블록, 문서화된 안전성 불변량; 퍼즈 테스트 |
| mio API 변경 | 낮음 | mio는 안정 (0.8+); 버전 고정 |
| 높은 코드 복잡도 | 수용 | 요구사항에 명시된 대로 성능을 위한 트레이드오프 |

---

## 부록 A: massdns 아키텍처 비교

### massdns가 하는 것 (7,132줄 C)

| 컴포넌트 | 줄 수 | 우리의 접근 | 우리 줄 수 (추정) |
|---|---|---|---|
| DNS wire format (전체) | ~1,750 | 최소 부분집합 | ~100 |
| 해시맵 (in-flight) | ~340 | 직접 인덱싱 배열 | ~80 |
| Timed ring (타임아웃) | ~155 | Timing wheel | ~120 |
| 이벤트 루프 (epoll) | ~500 | mio + sendmmsg/recvmmsg | ~300 |
| 재시도 / 리졸버 로테이션 | ~100 | 동일한 로직 | ~60 |
| 소켓 설정 / 버퍼 | ~200 | socket2 + 배치 I/O 설정 | ~150 |
| 멀티프로세스 fork | ~300 | 불필요 | 0 |
| TCP 지원 | ~400 | 불필요 | 0 |
| Raw 소켓 / IPv6 소스 | ~200 | 불필요 | 0 |
| 출력 포맷 | ~500 | 불필요 (내부 API) | 0 |
| CLI / stats / 권한 | ~800 | 불필요 | 0 |
| 공개 API + 판정 | 0 | 신규 | ~80 |
| 플랫폼 추상화 | 0 | 신규 (sendmmsg/폴백) | ~200 |
| **합계** | **~5,245** (관련) | | **~1,090** |

### massdns 대비 추가하는 것 (개념적)

1. **sendmmsg/recvmmsg 배치 I/O** -- massdns는 이것을 사용하지 않음
2. **해시맵 대신 직접 인덱싱 배열** -- 더 빠른 조회
3. **다중 소켓** 라운드 로빈 -- massdns는 기본 1개
4. **크로스 플랫폼** mio 추상화 -- massdns는 Linux/macOS만
