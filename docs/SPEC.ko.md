# domaingrep - 기술 사양서

> 대량 도메인 가용성 검색 CLI 도구.

**버전:** 0.2.4
**최종 업데이트:** 2026-04-06

---

## 목차

1. [개요](#1-개요)
2. [아키텍처](#2-아키텍처)
3. [CLI 인터페이스](#3-cli-인터페이스)
4. [입력 파싱 및 검증](#4-입력-파싱-및-검증)
5. [Domain Hack 감지](#5-domain-hack-감지)
6. [TLD 관리](#6-tld-관리)
7. [캐시 시스템 (1-3글자 도메인)](#7-캐시-시스템-1-3글자-도메인)
8. [UDP DNS 조회 (4글자 이상)](#8-udp-dns-조회-4글자-이상)
9. [출력 포맷](#9-출력-포맷)
10. [자동 업데이트](#10-자동-업데이트)
11. [캐시 빌더 및 GitHub Actions](#11-캐시-빌더-및-github-actions)
12. [배포 및 설치](#12-배포-및-설치)
13. [프로젝트 구조](#13-프로젝트-구조)
14. [테스트 전략](#14-테스트-전략)
15. [CI/CD 파이프라인](#15-cicd-파이프라인)
16. [에러 처리](#16-에러-처리)
17. [성능 메모](#17-성능-메모)
18. [의존성](#18-의존성)

---

## 1. 개요

### 1.1 domaingrep이 하는 일

`domaingrep`은 하나의 입력 라벨을 많은 TLD와 조합하여, 등록 가능해 보이는 도메인을 빠르게 찾는 CLI 도구입니다.

이 구현은 대량 후보 탐색에 최적화되어 있으며, 레지스트라 수준의 완벽한 구매 가능성 판정은 목표로 하지 않습니다. 예약, 프리미엄, 정책 차단 도메인은 CLI 결과와 다를 수 있습니다.

### 1.2 2단계 조회 전략

| 도메인 길이 | 방법 | 소스 |
|---|---|---|
| 1-3글자 | 비트맵 캐시 조회 | 로컬 `cache.bin` |
| 4글자 이상 | 실시간 UDP DNS NS 질의 | 공용 Rust resolver 엔진 |

도메인 hack의 경우, 원본 입력 길이가 아니라 hack으로 분리된 SLD 길이에 따라 방법을 결정합니다.

### 1.3 외부 동작 특징

- 비대화형 CLI만 제공
- 기본 출력은 일반 텍스트, `--json` 시 NDJSON
- 기본은 available만 표시, `--all` 시 unavailable 포함
- 캐시 갱신과 업데이트 체크는 best-effort 백그라운드 작업이며 메인 쿼리를 막지 않음
- DNS 질의 실패는 partial skip으로 노출하지 않고 `unavailable`로 접음

---

## 2. 아키텍처

### 2.1 실행 흐름

```text
domaingrep <DOMAIN>
    |
    v
[1] 입력 파싱 및 검증
    |
    v
[2] 백그라운드 작업 시작
    |--- 캐시 신선도 체크 (best effort)
    |--- CLI 업데이트 체크 (best effort)
    |
    v
[3] 캐시 로드
    |
    v
[4] Domain hack 해석 (mode A 전용)
    |--- 1-3글자 hack SLD -> 비트맵 캐시
    |--- 4글자 이상 hack SLD -> UDP DNS resolver
    |
    v
[5] 일반 TLD 해석
    |--- 1-3글자 SLD -> 비트맵 캐시
    |--- 4글자 이상 SLD -> UDP DNS resolver
    |
    v
[6] 정렬 및 포맷
    |
    v
[7] stdout 출력, 이후 선택적 stderr 노트
```

### 2.2 현재 resolver 구조

실시간 resolver는 다음 위치에서 공용으로 사용되는 Rust 네이티브 UDP stub resolver입니다.

- CLI의 4글자 이상 조회
- cache-builder의 1-3글자 캐시 생성
- `cache-builder fetch-tlds`의 TLD probe 단계

resolver는 현재 다음으로 구성됩니다.

- `src/resolve/wire.rs`의 DNS wire-format 패킷 생성
- `socket2` 기반 non-blocking UDP socket
- readiness polling용 `mio::Poll`
- DNS transaction ID로 직접 인덱싱하는 lookup slab
- retry/timeout용 timing wheel

DoH, `reqwest`, 외부 `massdns` subprocess는 사용하지 않습니다.

### 2.3 환경변수 기반 런타임 설정

CLI 런타임이 지원하는 환경변수:

| 변수 | 의미 |
|---|---|
| `DOMAINGREP_CACHE_DIR` | 캐시 디렉터리 오버라이드 |
| `DOMAINGREP_CACHE_URL` | 캐시 에셋 URL 오버라이드 |
| `DOMAINGREP_CACHE_CHECKSUM_URL` | 체크섬 URL 오버라이드 |
| `DOMAINGREP_UPDATE_API_URL` | GitHub 최신 릴리스 API URL 오버라이드 |
| `DOMAINGREP_RESOLVERS` | resolver 목록 오버라이드 (쉼표/공백 구분) |
| `DOMAINGREP_RESOLVE_CONCURRENCY` | 실시간 resolver 동시성 오버라이드 |
| `DOMAINGREP_RESOLVE_TIMEOUT_MS` | 시도당 timeout(ms) 오버라이드 |
| `DOMAINGREP_RESOLVE_ATTEMPTS` | 도메인당 최대 시도 횟수 오버라이드 |
| `DOMAINGREP_RESOLVE_SOCKET_COUNT` | UDP socket 수 오버라이드 |
| `DOMAINGREP_DISABLE_UPDATE` | 백그라운드 버전 체크 비활성화 |

`DOMAINGREP_RESOLVERS`는 `1.1.1.1`, `8.8.8.8:53` 같은 IP/소켓 주소를 혼합해서 받을 수 있습니다.

---

## 3. CLI 인터페이스

### 3.1 사용법

```text
domaingrep [OPTIONS] <DOMAIN>
```

### 3.2 위치 인수

| 인수 | 설명 |
|---|---|
| `<DOMAIN>` | 검색 대상. `abc` 또는 `abc.sh` 지원. 정확히 하나만 허용. |

### 3.3 플래그 및 옵션

| 플래그 | 단축 | 기본값 | 설명 |
|---|---|---|---|
| `--all` | `-a` | `false` | unavailable 결과도 표시 |
| `--json` | `-j` | `false` | NDJSON 출력 |
| `--tld-len <RANGE>` | `-t` | 전체 | TLD 길이 필터: `2`, `2..5`, `..3`, `4..` |
| `--limit <N>` | `-l` | 터미널에서는 `25`, 그 외에는 없음 | 필터링 후 최대 출력 행 수. `0`은 무제한을 의미합니다. |
| `--color <WHEN>` | | `auto` | `auto`, `always`, `never` |
| `--help` | `-h` | | 도움말 표시 |
| `--version` | `-V` | | 버전 표시 |

### 3.4 종료 코드

| 코드 | 의미 |
|---|---|
| `0` | available 결과가 하나 이상 출력됨 |
| `1` | available 결과가 없음 |
| `2` | 잘못된 입력, 캐시/설정/부트스트랩 네트워크 에러 |

중요: 도메인별 DNS timeout이나 비확정 응답은 exit code `2`를 만들지 않습니다. 해당 도메인은 `unavailable`로 접히며, 결과적으로 `1`이 될 수 있습니다.

---

## 4. 입력 파싱 및 검증

### 4.1 모드

**Mode A: SLD only**

```text
domaingrep abc
```

파싱 결과:

- `sld = "abc"`
- `tld_prefix = None`
- domain hack 감지 활성화

**Mode B: SLD + TLD prefix**

```text
domaingrep abc.sh
```

파싱 결과:

- `sld = "abc"`
- `tld_prefix = Some("sh")`
- domain hack 감지 비활성화

### 4.2 정규화 규칙

1. 입력은 자동으로 소문자화됩니다.
2. 단일 trailing dot은 dot 개수 검증 전에 제거됩니다.
3. trailing-dot 제거 후 dot은 최대 1개만 허용됩니다.

예시:

```text
ABC      -> abc
abc.     -> abc
abc.co.  -> sld=abc, prefix=co
abc.co.uk -> error
```

### 4.3 라벨 검증 규칙

SLD와 선택적 TLD prefix 모두 다음을 만족해야 합니다.

1. 허용 문자: `[a-z0-9-]`
2. 최소 길이: 1
3. 최대 길이: 63
4. `-`로 시작 불가
5. `-`로 끝 불가
6. 3-4번째 위치에 `--` 불가

### 4.4 에러 포맷

에러는 다음 형태를 사용합니다.

```text
error: <message>
  --> <선택적 context>
  = help: <선택적 suggestion>
```

`-->` 줄은 구현이 유의미한 위치/컨텍스트 문자열을 갖고 있을 때만 출력됩니다.

예시:

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

## 5. Domain Hack 감지

### 5.1 동작

mode A 입력에 대해 입력 문자열의 suffix를 알려진 TLD 집합과 매칭합니다.

예시:

```text
입력: bunsh
매치: bun.sh
```

### 5.2 규칙

1. 필터링된 알려진 TLD만 고려합니다.
2. TLD 앞의 SLD 부분은 최소 1글자여야 합니다.
3. 파생된 SLD는 일반 입력과 같은 라벨 검증을 통과해야 합니다.
4. 결과는 SLD 길이 오름차순으로 정렬됩니다.
   즉, 더 긴 TLD 매치가 먼저 나옵니다.
5. Domain hack 결과는 항상 일반 결과보다 먼저 출력됩니다.
6. mode B (`abc.sh`)에서는 hack 감지가 비활성화됩니다.
7. Hack 결과도 `--limit`에 포함됩니다.

### 5.3 조회 방법

| Hack SLD 길이 | 방법 |
|---|---|
| 1-3 | 캐시 조회 |
| 4+ | 실시간 UDP DNS 조회 |

---

## 6. TLD 관리

### 6.1 소스

- HTTP source: `https://tld-list.com/df/tld-list-details.json`

### 6.2 필터링 규칙

다음 조건을 모두 만족하는 TLD만 포함됩니다.

1. ASCII 소문자 key만 사용
2. `punycode`가 `null`
3. `type != "infrastructure"`
4. 공개 등록 probe 통과

### 6.3 공개 등록 probe

빌더는 공용 UDP resolver를 두 단계로 사용합니다.

1. `nic.{tld}`에 `NS` 질의
   - `RCODE == NOERROR` 이고 `answer_count > 0` 인 경우만 통과
2. `xyzzy-probe-test-{random}.{tld}`에 `NS` 질의
   - `RCODE == NXDOMAIN` 인 경우만 포함

Timeout 또는 비확정 결과는 해당 TLD를 제외합니다.

### 6.4 정렬

일반 결과는 다음 순서로 정렬됩니다.

1. TLD 길이 오름차순
2. 하드코딩된 인기도 순서
3. 알파벳 순

### 6.5 `--tld-len`

지원 문법:

| 입력 | 의미 |
|---|---|
| `2` | 정확히 2 |
| `2..5` | 양끝 포함 범위 |
| `..3` | 3 이하 |
| `4..` | 4 이상 |

---

## 7. 캐시 시스템 (1-3글자 도메인)

### 7.1 범위

캐시는 필터링된 TLD 집합 전체에 대해 유효한 1-3글자 라벨의 availability bit를 저장합니다.

### 7.2 도메인 공간

| 길이 | 개수 |
|---|---|
| 1 | 36 |
| 2 | 1,296 |
| 3 | 47,952 |
| 총합 | 49,284 |

인덱싱 문자 규칙:

- edge 위치: `[a-z0-9]`
- 3글자 라벨의 middle 위치: `[a-z0-9-]`

### 7.3 파일 포맷

`cache.bin`은 다음으로 구성됩니다.

1. magic bytes: `DGRP`
2. format version: `u16 LE`
3. build timestamp: `i64 LE`
4. TLD count: `u16 LE`
5. bitmap payload의 SHA-256
6. 가변 길이 TLD index table
7. bitmap payload

bitmap 순서:

1. TLD index
2. 해당 TLD 내 domain index

`1`은 available, `0`은 unavailable 입니다.

### 7.4 로컬 저장소

기본 캐시 디렉터리:

- Linux: `~/.cache/domaingrep/`
- macOS: `~/Library/Caches/domaingrep/`
- Windows: `%LOCALAPPDATA%/domaingrep/`

파일:

```text
cache.bin
cache.meta
last_update_check
```

### 7.5 라이프사이클

1. `cache.bin`이 존재하고 파싱되면 즉시 사용합니다.
2. stale (>=24h) 이면 그대로 사용하면서 백그라운드 refresh를 시작합니다.
3. 없거나 손상되었으면 `cache.bin.gz`와 `cache.sha256`를 다운로드합니다.
4. 체크섬 검증 후 압축을 해제하고 로컬 파일을 원자적으로 교체합니다.
5. 로컬 캐시가 없고 다운로드도 실패하면 exit code `2`로 종료합니다.

short-domain 조회는 캐시를 부트스트랩할 수 없을 때 live DNS로 폴백하지 않습니다.

---

## 8. UDP DNS 조회 (4글자 이상)

### 8.1 질의 모델

- 레코드 타입: `NS`
- 전송: 설정된 recursive resolver로 UDP 질의
- DoH 사용 안 함
- TCP 폴백 없음

### 8.2 기본 resolver 목록

기본 내장 resolver:

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

### 8.3 판정 규칙

공용 판정 규칙:

| 결과 | 의미 |
|---|---|
| `NXDOMAIN` | available |
| `NOERROR` | unavailable |
| 기타 RCODE | 재시도 후 소진되면 unavailable |
| timeout / 무응답 | 재시도 후 소진되면 unavailable |

외부에 노출되는 `unknown` 상태는 없습니다.

### 8.4 기본값

CLI 기본값:

- concurrency: `1000`
- timeout: `500ms`
- max attempts: `4`
- socket count: `1`

Builder 기본값:

- concurrency: `10000`
- timeout: `500ms`
- max attempts: `4`
- socket count: Linux에서는 `4`, 그 외는 `1`

### 8.5 내부 엔진 동작

실시간 resolver는 다음을 수행합니다.

1. DNS wire-format NS query packet 생성
2. 고유 transaction ID 할당
3. non-blocking UDP socket으로 패킷 전송
4. transaction ID 기반 slab에 in-flight lookup 저장
5. timing wheel로 timeout과 retry 관리
6. 확정된 `rcode`/`answer_count` 또는 `None` 반환

CLI 가시 출력에서는 `None`을 unavailable로 처리합니다.

---

## 9. 출력 포맷

### 9.1 일반 텍스트

기본 출력은 available 결과만 표시합니다.

```text
bun.sh
bunsh.io
bunsh.dev
```

`--all` 사용 시:

```text
  bun.sh
x bunsh.io
  bunsh.dev
```

### 9.2 JSON

`--json`은 NDJSON을 출력합니다. 각 줄은 하나의 객체입니다.

```json
{"domain":"bun.sh","available":true,"kind":"hack","method":"cache"}
{"domain":"bunsh.io","available":false,"kind":"regular","method":"dns"}
```

필드:

- `domain`
- `available`
- `kind`: `hack` 또는 `regular`
- `method`: `cache` 또는 `dns`

### 9.3 순서

1. 모든 hack 결과 먼저
2. 이후 일반 결과는 TLD 길이, 인기도, 알파벳 순으로 정렬

### 9.4 `--limit`

`--limit`는 visibility filtering 이후에 적용됩니다.

- `--all` 없음: unavailable 제거 후 truncate
- `--all` 있음: available/unavailable 모두 포함한 뒤 truncate
- `--limit 0`은 truncate를 비활성화합니다
- stdout이 터미널이면 plain text 출력은 `--limit`를 생략했을 때 기본적으로 `25`행만 표시합니다
- stdout이 터미널이 아니거나 `--json`을 사용할 때는 `--limit`를 생략하면 무제한입니다

DNS 작업 자체는 truncate 전에 모두 수행됩니다.

### 9.5 stderr 노트

현재 stderr 노트:

- `note: no available domains found for '{input}'`
- plain text 출력이 잘린 경우 `note: {remaining} more domains not shown (showing {shown} of {total}; use --limit 0 to show all)`
- 백그라운드 update check가 종료 전에 끝났을 경우의 업데이트 노트

현재 구현은 partial DNS failure note를 출력하지 않습니다.

---

## 10. 자동 업데이트

### 10.1 버전 체크

`last_update_check`가 없거나 24시간 이상 지난 경우:

1. 백그라운드 GitHub API 요청 시작
2. 최신 릴리스 메타데이터 조회
3. 현재 버전과 릴리스 태그 비교
4. 더 새 버전이고 종료 전에 완료되면 stderr 노트 출력
5. `last_update_check` 갱신

### 10.2 보장 사항

- best effort only
- 메인 출력 지연 없음
- 최대 24시간에 한 번

---

## 11. 캐시 빌더 및 GitHub Actions

### 11.1 명령

`cache-builder` 바이너리는 다음 세 명령을 제공합니다.

```text
cache-builder fetch-tlds
cache-builder scan --tlds <...>
cache-builder merge --output <PATH>
```

### 11.2 `fetch-tlds`

책임:

1. 소스 URL에서 TLD JSON fetch
2. 공용 resolver 엔진으로 공개 등록 가능 여부 probe
3. TLD 정렬 및 그룹 분할
4. GitHub Actions matrix용 JSON 출력

### 11.3 `scan`

책임:

1. 모든 short-domain 생성
2. 공용 UDP DNS 엔진으로 `{domain}.{tld}`를 chunk 단위 조회
3. available 결과에 해당하는 bitmap bit 설정
4. `partial-bitmap.bin` 기록

이 명령은 더 이상 `massdns`에 의존하지 않습니다.

### 11.4 `merge`

책임:

1. partial bitmap 파일 수집
2. TLD slice를 하나의 cache file로 병합
3. 최종 `cache.bin` 기록

### 11.5 GitHub Actions workflow

현재 cache build workflow:

1. `cache-builder` build
2. `fetch-tlds` 실행
3. TLD group별 matrix scan (`cache-builder scan`)
4. partial bitmap merge
5. 최종 cache gzip + checksum 생성
6. `cache-latest` release asset publish

---

## 12. 배포 및 설치

저장소에는 다음 배포 자동화가 포함되어 있습니다.

- GitHub release archive
- crates.io publish
- npm wrapper/platform package publish
- Homebrew tap update

Shell install script는 release 시 렌더링되어 release asset으로 함께 게시됩니다.

---

## 13. 프로젝트 구조

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

## 14. 테스트 전략

### 14.1 테스트 범주

| 범주 | 커버 내용 |
|---|---|
| 입력 검증 | normalization, 에러, range parsing |
| Hack 감지 | suffix matching, ordering |
| Cache | bitmap index, parsing, download/update behavior |
| Resolver | wire-format parsing, UDP retry, timeout collapse |
| Output | plain text, NDJSON formatting |
| Update | GitHub release check behavior |
| CLI | end-to-end output, exit code |
| Live smoke | public resolver 대상 ignored test |

### 14.2 Mock DNS 전략

Resolver integration test는 loopback UDP mock DNS server를 사용합니다. 이 서버는:

- 들어온 question name을 파싱하고
- 원하는 `rcode`와 `answer_count`를 반환하며
- retry 동작 검증을 위해 의도적으로 패킷을 drop 할 수 있습니다

---

## 15. CI/CD 파이프라인

### 15.1 CI workflow

현재 CI는 다음을 실행합니다.

1. `cargo clippy --all-targets -- -D warnings`
2. `cargo fmt --check`
3. `cargo test --all-features --lib --bins --test cli --test cache --test resolve --test hack --test output --test update`
4. `cargo test --test live_dns_smoke -- --ignored`
5. `cargo build --release`
6. `cargo llvm-cov --ignore-filename-regex '(^|.*/)bin/cache_builder\.rs$' --fail-under-lines 75`

### 15.2 Release workflow

Release workflow는 설정된 target matrix에 대해 release binary를 빌드하고, release asset/crate/npm/Homebrew publish 단계를 수행합니다.

---

## 16. 에러 처리

### 16.1 주요 시나리오

| 시나리오 | Exit |
|---|---|
| 잘못된 입력 | `2` |
| 캐시 bootstrap/download 실패 | `2` |
| 캐시 checksum mismatch | `2` |
| resolver 설정 오류 | `2` |
| available 결과 없음 | `1` |

### 16.2 현재 DNS 실패 정책

도메인별 live DNS 실패는 hard command error로 표면화되지 않습니다.

대신 다음과 같이 처리합니다.

1. 시도 횟수 소진까지 retry
2. 끝까지 unresolved면 `unavailable`로 접음
3. 일반 출력 흐름 계속 진행

즉, 현재 CLI는 per-run `"N TLDs could not be checked"` 노트를 출력하지 않습니다.

---

## 17. 성능 메모

- warm-cache short-domain 조회는 사실상 즉시 수행됩니다.
- 4글자 이상 조회는 resolver RTT와 resolver 상태가 지배적입니다.
- resolver는 per-query HTTP/TLS/JSON 오버헤드를 피합니다.
- builder 처리량은 CPU보다 네트워크와 resolver 상태에 더 크게 좌우됩니다.

이 사양서는 특정 benchmark 숫자를 안정된 계약으로 간주하지 않습니다.

---

## 18. 의존성

### 18.1 핵심 크레이트

| 크레이트 | 목적 |
|---|---|
| `clap` | CLI 파싱 |
| `tokio` | main entrypoint와 백그라운드 task용 async runtime |
| `reqwest` | 캐시 다운로드, 업데이트 체크, TLD 리스트 fetch |
| `serde` / `serde_json` | 메타데이터 및 JSON 파싱 |
| `flate2` | cache gzip 해제 |
| `sha2` | SHA-256 checksum 검증 |
| `dirs` | 플랫폼 캐시 디렉터리 조회 |
| `memmap2` | 로컬 캐시 파일 memory-map |
| `is-terminal` | color용 TTY 감지 |
| `mio` | live UDP DNS resolver의 readiness polling |
| `socket2` | UDP socket 생성 및 socket option 설정 |
| `rand` | transaction ID 및 probe name 생성 |
| `semver` | update-version 비교 |

### 18.2 의존성 분리

- HTTP 스택: cache/update/TLD source 전용
- UDP resolver: `src/resolve/`의 custom resolver 엔진
- DoH client 없음
- 외부 `massdns` 의존성 없음
