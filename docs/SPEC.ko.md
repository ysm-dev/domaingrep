# domaingrep - 기술 사양서

> 대량 도메인 가용성 검색 CLI 도구.

**버전:** 0.1.0
**최종 업데이트:** 2026-03-29

---

## 목차

1. [개요](#1-개요)
2. [아키텍처](#2-아키텍처)
3. [CLI 인터페이스](#3-cli-인터페이스)
4. [입력 파싱 및 검증](#4-입력-파싱-및-검증)
5. [Domain Hack 감지](#5-domain-hack-감지)
6. [TLD 관리](#6-tld-관리)
7. [캐시 시스템 (1-3글자 도메인)](#7-캐시-시스템-1-3글자-도메인)
8. [실시간 DNS 조회 (4글자 이상)](#8-실시간-dns-조회-4글자-이상)
9. [출력 포맷](#9-출력-포맷)
10. [자동 업데이트](#10-자동-업데이트)
11. [캐시 빌더 (GitHub Actions)](#11-캐시-빌더-github-actions)
12. [배포 및 설치](#12-배포-및-설치)
13. [프로젝트 구조](#13-프로젝트-구조)
14. [테스트 전략](#14-테스트-전략)
15. [CI/CD 파이프라인](#15-cicd-파이프라인)
16. [에러 핸들링](#16-에러-핸들링)
17. [성능 목표](#17-성능-목표)
18. [의존성](#18-의존성)

---

## 1. 개요

### 1.1 domaingrep이란?

`domaingrep`은 도메인 이름의 가용성을 극한의 속도로 대량 검색하는 CLI 도구입니다. 주어진 도메인명(SLD)을 모든 알려진 TLD와 조합하여 빠른 DNS 기반 체크로 등록 가능해 보이는 도메인을 보고합니다.

### 1.2 핵심 철학

- **즉시 속도** - 1-3글자 도메인은 사전 구축된 bitmap 캐시로, 4글자 이상은 병렬 DNS로
- **Zero config** - 훌륭한 기본값, 설정 불필요
- **비대화형** - 순수 비대화형 CLI, 파이프 친화적
- **자동 업데이트** - CLI와 캐시가 자동으로 최신 상태 유지
- **크로스 플랫폼** - macOS, Linux, Windows

### 1.3 핵심 혁신

**2단계 가용성 체크:**

| 도메인 길이 | 방법 | 속도 |
|---|---|---|
| 1-3글자 | 사전 구축된 bitmap 캐시 (매일 갱신) | O(1), 즉시 |
| 4글자 이상 | 실시간 DNS-over-HTTPS 쿼리 | ~2-5초 병렬 |

bitmap 캐시는 모든 가능한 1-3글자 도메인의 일일 DNS 기반 가용성 스냅샷을 전체 TLD에 대해 저장합니다. GitHub Actions로 매일 재구축되며 GitHub Release 에셋으로 저장됩니다 (gzip 압축 ~1-2MB).

빠른 대량 후보 탐색에 최적화되어 있으며, 레지스트라 수준의 완벽한 정확도를 보장하지는 않습니다. 예약/프리미엄/레지스트리 블록된 도메인은 CLI 결과와 다를 수 있습니다.

---

## 2. 아키텍처

### 2.1 실행 흐름

```
domaingrep <입력>
    |
    v
[1. 입력 파싱 및 검증]
    |
    v
[2. 동시 작업] ----+---- [캐시 신선도 체크 (백그라운드)]
    |              |
    |              +---- [CLI 버전 체크 (백그라운드)]
    |
    v
[3. 가용성 조회]
    |--- 1-3글자: Bitmap 캐시 조회 (O(1))
    |--- 4글자+:  Cloudflare/Google DNS에 병렬 DoH 쿼리
    |
    v
[4. Domain Hack 감지]
    |--- 유효한 TLD suffix 분할 찾기
    |--- 각 hack에 대해 가용성 체크
    |
    v
[5. 정렬 및 포맷]
    |--- Domain hack이 상단
    |--- 이후: TLD 길이 오름차순, 같은 길이면 인기도순
    |
    v
[6. stdout 출력]
    |--- 일반 텍스트 (기본) 또는 JSON (--json)
```

### 2.2 데이터 흐름도

```
GitHub Actions (매일 cron)
    |
    v
[캐시 빌더] ---> [GitHub Release 에셋]
    |                      |
    |  (tld-list.com)      | (HTTP 다운로드)
    |                      |
    v                      v
[TLD 리스트 JSON]    [CLI: domaingrep]
                       |
                       +---> XDG_CACHE_HOME/domaingrep/
                        |         |- cache.bin          (bitmap, 압축 해제됨)
                        |         |- cache.meta         (메타데이터 + 에셋 체크섬)
                        |         |- last_update_check  (타임스탬프)
                       |
                       +---> Cloudflare DoH (4글자 이상)
                       |         |- 주: cloudflare-dns.com
                       |         |- 대체: dns.google
                       |
                       v
                   [stdout: 결과]
```

---

## 3. CLI 인터페이스

### 3.1 사용법

```
domaingrep [OPTIONS] <DOMAIN>
```

### 3.2 위치 인수

| 인수 | 설명 |
|---|---|
| `<DOMAIN>` | 검색할 도메인. SLD만 (`abc`) 또는 SLD+TLD prefix (`abc.sh`) 가능. 단일 도메인만. |

### 3.3 플래그 및 옵션

| 플래그 | 단축 | 기본값 | 설명 |
|---|---|---|---|
| `--all` | `-a` | `false` | 사용 불가 도메인도 표시 (기본: 사용 가능만) |
| `--json` | `-j` | `false` | JSON 출력 (줄당 하나의 객체, NDJSON) |
| `--tld-len <RANGE>` | `-t` | (전체) | TLD 길이 필터. 지원: `2` (정확히 2), `2..5` (2~5 포함), `..3` (3 이하), `4..` (4 이상) |
| `--limit <N>` | `-l` | (없음) | 필터링 후 출력할 최대 행 수. Domain hack도 총 카운트에 포함. DNS 요청은 모두 수행 (조기 종료 없음). |
| `--color <WHEN>` | | `auto` | 색상 출력: `auto`, `always`, `never`. auto는 TTY 감지. |
| `--help` | `-h` | | 도움말 표시 |
| `--version` | `-V` | | 버전 표시 |

### 3.4 사용 예시

```bash
# 'abc'를 모든 TLD에 대해 검색 (사용 가능만)
domaingrep abc

# TLD prefix 매칭: 'sh'가 .sh, .shop, .show 등과 매칭
domaingrep abc.sh

# 사용 불가 도메인 포함 전체 결과
domaingrep abc --all

# JSON 출력
domaingrep abc --json

# TLD 길이 2-3글자만
domaingrep abc --tld-len 2..3

# 사용 가능 결과 10개로 제한
domaingrep abc --limit 10

# Domain hack 감지: 'bunsh' 입력에서 'bun.sh' 발견
domaingrep bunsh

# 플래그 조합
domaingrep myapp.de --tld-len ..4 --limit 20 --json

# 복수 검색 (권장 방법)
domaingrep abc; domaingrep xyz
```

### 3.5 종료 코드

| 코드 | 의미 |
|---|---|
| `0` | 사용 가능한 도메인 1개 이상 발견 (`grep` 관례) |
| `1` | 사용 가능한 도메인 없음 |
| `2` | 에러 (잘못된 입력, 네트워크 실패 등) |

---

## 4. 입력 파싱 및 검증

### 4.1 입력 모드

입력 `<DOMAIN>`은 두 가지 모드 중 하나로 파싱됩니다:

**모드 A: SLD만** (입력에 점 없음)
```
domaingrep abc     -> SLD = "abc", TLD = 전체
domaingrep bunsh   -> SLD = "bunsh", TLD = 전체 (+ domain hack 감지)
```

**모드 B: SLD + TLD Prefix** (점 포함)
```
domaingrep abc.sh  -> SLD = "abc", TLD 필터 = prefix "sh" (.sh, .shop, .show 등 매칭)
domaingrep abc.com -> SLD = "abc", TLD 필터 = prefix "com" (.com, .community 등 매칭)
domaingrep abc.    -> trailing dot 무시, "abc"로 처리 (모드 A)
```

### 4.2 검증 규칙

1. **허용 문자:** `[a-z0-9-]` (소문자 변환 후)
2. **자동 소문자 변환:** `ABC` -> `abc` (무음, 경고 없음)
3. **Trailing dot 제거:** `abc.` -> `abc`
4. **하이픈 규칙 (LDH 문법 + punycode 예약 패턴):**
   - 하이픈으로 시작 불가: `-abc` -> 에러
   - 하이픈으로 끝 불가: `abc-` -> 에러
   - 3-4번째 위치 하이픈 불가 (IDN 처리의 `xn--`/A-label 예약): `ab--c` -> 에러
5. **길이 제한:**
   - SLD 최소: 1글자
   - SLD 최대: 63글자
6. **점 개수:** `<DOMAIN>`에 점 최대 1개; `abc.co.uk` 같은 다중 레이블 입력 -> 에러
7. **빈 입력:** 에러
8. **잘못된 문자:** 에러

### 4.3 검증 에러 형식

모든 에러는 Why / Where / How to Fix 패턴을 따릅니다:

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

## 5. Domain Hack 감지

### 5.1 알고리즘

입력 문자열 `S` (점 없음)에서, `S`의 suffix 중 알려진 TLD와 매칭되는 모든 경우를 찾습니다:

```
입력: "bunsh"

suffix 스캔:
  "bunsh" -> TLD "bunsh"? 아니오
  "unsh"  -> TLD "unsh"?  아니오
  "nsh"   -> TLD "nsh"?   아니오
  "sh"    -> TLD "sh"?    예 -> "bun.sh"
  "h"     -> TLD "h"?     아니오

결과: domain hack "bun.sh" 감지
```

### 5.2 규칙

1. **유효하고 실제 존재하는 TLD**만 매칭
2. SLD 부분 (매칭된 TLD 앞)은 **최소 1글자** 이상
3. SLD 부분은 **유효**해야 함 (4.2와 동일한 규칙)
4. **우선순위:** 짧은 SLD 우선 (즉, 가장 긴 매칭 TLD 우선)
   - `domaingrep openai` -> `ope.nai` (.nai가 있다면)이 `opena.i` (.i가 있다면)보다 우선
5. Domain hack 결과는 출력 **최상단**에 일반 결과 앞에 배치
6. Domain hack 감지는 일반 TLD 검색에 **추가**로 실행
   - `domaingrep bunsh`는 `bun.sh` (hack)과 `bunsh.com`, `bunsh.net` 등 (일반) 모두 표시
7. **모드 B (점 포함): Domain hack 감지 비활성화**
   - `domaingrep bunsh.sh`는 `bun.sh`를 hack으로 감지하지 않음; `bunsh`를 TLD prefix `sh`로만 검색
8. **Hack 결과의 가용성 체크 소스:** hack 분할의 SLD가 체크 방법을 결정:
   - SLD 1-3글자 -> bitmap 캐시 조회 (예: `bun.sh` -> "bun"은 3글자 -> 캐시)
   - SLD 4글자 이상 -> 실시간 DNS 쿼리 (예: `domaingre.p` -> "domaingre"는 9글자 -> DNS)
9. **--limit 상호작용:** Domain hack 결과는 `--limit` 총 카운트에 포함
   - `--limit 5`에 hack 3개 발견 -> hack 3 + 일반 2 = 총 5개 표시

### 5.3 TLD suffix 매칭용 데이터 구조

효율적 suffix 매칭을 위해 **역방향 trie** 사용:

```
TLD 리스트: [sh, shop, show, com, co, ...]

역방향 trie:
  h -> s -> (매칭: "sh")
       -> p -> o -> (매칭: "shop")
  ...
```

입력 "bunsh"에 대해:
1. 역순: "hsnub"
2. 역방향 trie에서 'h'부터 탐색
3. 깊이 2에서 매칭 발견: "sh" -> 분할: "bun" + "sh"

---

## 6. TLD 관리

### 6.1 TLD 소스

- **주:** `https://tld-list.com/df/tld-list-details.json`
- **갱신:** 매일, 캐시 빌드 시 (GitHub Actions)

### 6.2 TLD 필터링 기준

tld-list.com JSON에서 **모든 조건**을 만족하는 TLD만 포함:

1. **ASCII만:** `punycode` 필드가 `null`이고 TLD 키가 `[a-z]` 문자만 포함 (구매 불가한 숫자 TLD 제외)
2. **인프라 아님:** `type`이 `"infrastructure"`가 아님 (`.arpa` 제외)
3. **공개 등록 가능:** 일반 대중이 등록 가능한 TLD (브랜드 전용 TLD 제외). 캐시 빌드 시 **probe 테스트**로 판단:
   - 1단계: `nic.{tld}`에 NS 레코드 조회. NS 레코드 없으면 비활성 TLD -> 제외.
   - 2단계: `xyzzy-probe-test-{random}.{tld}`에 NS 레코드 조회. NXDOMAIN이면 공개 등록 지원 -> 포함. NOERROR(와일드카드)나 SERVFAIL(3회 재시도 후)이면 제외.
   - 이를 통해 브랜드 TLD (예: `.google`, `.apple`)를 수동 목록 없이 자동으로 필터링.

### 6.3 TLD 정렬

결과 정렬 순서:

1. **1차:** TLD 길이 오름차순 (`.io`가 `.com`보다, `.com`이 `.shop`보다 앞)
2. **2차 (같은 길이):** 하드코딩된 인기도 리스트 순

### 6.4 하드코딩된 인기 TLD 리스트

인기도 순 상위 ~50 TLD (2차 정렬 키로 사용):

```rust
const TLD_POPULARITY: &[&str] = &[
    // 길이 2
    "io", "ai", "co", "me", "to", "sh", "cc", "tv", "is", "so",
    "im", "ly", "fm", "am", "it", "us", "uk", "de", "fr", "nl",
    "be", "at", "ch", "se", "no", "fi", "dk", "jp", "kr", "in",
    "ca", "au", "nz", "za", "br", "mx",
    // 길이 3
    "com", "net", "org", "dev", "app", "xyz", "art", "fun", "icu", "top",
    "pro", "bio", "biz",
    // 길이 4+
    "info", "club", "site", "tech", "shop", "blog", "design",
];
```

인기 리스트에 없는 TLD는 인기 TLD 뒤에서 알파벳순으로 정렬.

### 6.5 --tld-len 범위 문법

`--tld-len` 플래그는 범위를 받습니다:

| 입력 | 의미 | 파싱 결과 |
|---|---|---|
| `2` | 정확히 길이 2 | `min=2, max=2` |
| `2..5` | 길이 2~5 (포함) | `min=2, max=5` |
| `..3` | 3 이하 (포함) | `min=1, max=3` |
| `4..` | 4 이상 | `min=4, max=MAX` |

사용자 친화적인 inclusive 범위 문법입니다: `2..5`는 2부터 5까지를 의미하며, Rust의 exclusive upper-bound 의미가 아닙니다.

---

## 7. 캐시 시스템 (1-3글자 도메인)

### 7.1 개요

모든 가능한 1-3글자 도메인의 전체 TLD 가용성을 매일 사전 체크하여 compact bitmap에 저장합니다. bitmap은 GitHub Release 에셋으로 배포되며 CLI가 첫 실행 시 다운로드합니다.

### 7.2 도메인 공간

유효 문자: 시작/끝 위치는 `[a-z0-9]` (36자), 중간 위치는 `[a-z0-9-]` (37자).

- 1글자 도메인: `[a-z0-9]`만 (하이픈 불가)
- 2글자 도메인: `[a-z0-9]` x `[a-z0-9]` (중간 위치 없음)
- 3글자 도메인: `[a-z0-9]` x `[a-z0-9-]` x `[a-z0-9]` (중간 위치에 하이픈 허용, 예: `a-b`)

| 길이 | 계산 | 개수 |
|---|---|---|
| 1글자 | 36 | 36 |
| 2글자 | 36 x 36 | 1,296 |
| 3글자 | 36 x 37 x 36 | 47,952 |
| **합계** | | **49,284** |

~1,200 TLD (필터링 후): 49,284 x 1,200 = ~59.1M 도메인-TLD 쌍.

### 7.3 Bitmap 포맷

#### 파일 구조

```
+----------------------------------+
| 헤더 (고정 크기)                  |
|   - 매직 바이트 (4B): "DGRP"    |
|   - 포맷 버전 (2B): u16         |
|   - 타임스탬프 (8B): i64 unix   |
|   - TLD 개수 (2B): u16          |
|   - 체크섬 (32B): SHA-256       |
+----------------------------------+
| TLD 인덱스 테이블                 |
|   - 각 TLD마다:                  |
|     - 길이 (1B): u8             |
|     - TLD 문자열 (가변)          |
+----------------------------------+
| Bitmap 데이터                     |
|   - 정렬: TLD 인덱스 순,        |
|     그 내에서 도메인 인덱스 순    |
|   - 도메인-TLD 쌍당 1비트        |
|   - 1 = 사용 가능               |
|   - 0 = 사용 불가               |
+----------------------------------+
```

#### 도메인 인덱스 계산

각 도메인은 결정적 인덱스에 매핑됩니다:

```rust
fn char_to_val(ch: char, allow_hyphen: bool) -> u32 {
    match ch {
        'a'..='z' => (ch as u32) - ('a' as u32),        // 0-25
        '0'..='9' => 26 + (ch as u32) - ('0' as u32),   // 26-35
        '-' if allow_hyphen => 36,                        // 36
        _ => unreachable!(), // 여기 도달 전에 검증됨
    }
}

fn domain_to_index(domain: &str) -> u32 {
    let chars: Vec<char> = domain.chars().collect();
    let len = chars.len();

    // 더 짧은 도메인에 대한 오프셋
    let offset: u32 = match len {
        1 => 0,                               // 1글자: 0부터 시작
        2 => 36,                              // 2글자: 36 이후
        3 => 36 + 1_296,                      // 3글자: 36 + 36*36 이후
        _ => unreachable!(),
    };

    // 같은 길이 그룹 내 위치 계산
    let index = match len {
        1 => char_to_val(chars[0], false),
        2 => {
            char_to_val(chars[0], false) * 36
            + char_to_val(chars[1], false)
        }
        3 => {
            // 첫째: 36자, 중간: 37자 (하이픈 포함), 마지막: 36자
            char_to_val(chars[0], false) * (37 * 36)
            + char_to_val(chars[1], true) * 36
            + char_to_val(chars[2], false)
        }
        _ => unreachable!(),
    };

    offset + index
}
```

#### 비트 조회

```rust
fn is_available(cache: &[u8], tld_index: usize, domain: &str) -> bool {
    let domain_index = domain_to_index(domain) as usize;
    let domains_per_tld = 49_284; // 전체 1-3글자 도메인 수 (36 + 1296 + 47952)
    let bit_position = tld_index * domains_per_tld + domain_index;
    let byte_offset = bit_position / 8;
    let bit_offset = bit_position % 8;
    (cache[byte_offset] >> (7 - bit_offset)) & 1 == 1
}
```

#### 크기 추정

- Bitmap: 49,284 도메인 x 1,200 TLD = 59,140,800 비트 = ~7.4 MB (원본)
- gzip 후: ~1-2 MB 추정 (sparse 데이터라 높은 압축률)

### 7.4 로컬 캐시 저장

**위치:** `dirs::cache_dir()` + `/domaingrep/`로 결정:
- **Linux:** `~/.cache/domaingrep/`
- **macOS:** `~/Library/Caches/domaingrep/`
- **Windows:** `%LOCALAPPDATA%/domaingrep/`

**파일:**
```
{cache_dir}/domaingrep/
  cache.bin            # 압축 해제된 bitmap 캐시 (조회/mmap용)
  cache.meta           # 메타데이터 JSON: {"format_version":1,"timestamp":1711670400,"asset_url":"..."}
  last_update_check    # 일반 텍스트 Unix 타임스탬프 (예: "1711670400")
```

**cache.meta 포맷 (JSON):**
```json
{
  "format_version": 1,
  "timestamp": 1711670400,
  "asset_url": "https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.bin.gz",
  "asset_sha256": "a1b2c3..."
}
```

**last_update_check 포맷:** Unix 타임스탬프(초)가 담긴 일반 텍스트 파일. 예: `1711670400`

### 7.5 캐시 생명주기

```
첫 실행:
  1. XDG_CACHE_HOME에서 기존 캐시 확인
  2. 캐시 없음 -> GitHub Releases에서 cache.bin.gz 다운로드
  3. 다운로드된 에셋의 SHA-256 체크섬 검증
  4. cache.bin으로 압축 해제 후 memory-map
  5. 조회 수행

이후 실행:
  1. 로컬 캐시 로드 (즉시)
  2. 메타데이터에서 캐시 나이 확인
  3. stale (>24시간): stale 캐시로 쿼리, 동시에 best-effort 백그라운드 갱신 시작
  4. 백그라운드 갱신: 쿼리와 함께 새 캐시 가져오기
  5. 다운로드 완료: 체크섬 검증, 임시 파일에 쓰기, 원자적 rename

로컬 캐시 없음 + 다운로드 실패:
  -> 에러 메시지 + exit 2 (짧은 도메인 DNS fallback 없음)

백그라운드 갱신 실패:
  -> 기존 로컬 캐시 계속 사용
  -> verbose/debug 빌드에서 선택적으로 stderr에 안내 출력

손상된 캐시:
  -> 로컬 캐시 삭제, GitHub Releases에서 재다운로드
  -> 재다운로드도 실패: 에러 + exit 2

동시 실행 인스턴스:
  -> 다운로드는 임시 파일 (cache.bin.{pid}.tmp)에 쓰기
  -> 완료 시 cache.bin으로 원자적 rename
  -> Unix: rename()은 원자적; Windows: ReplaceFile API 사용
  -> 읽기 측은 영향 없음 (시작 시 현재 cache.bin을 memory-map)
```

### 7.6 Stale-While-Revalidate

캐시는 stale-while-revalidate 전략을 사용합니다:

```
캐시 나이 < 24시간: 그대로 사용 (신선)
캐시 나이 >= 24시간: 즉시 사용 (stale), best-effort 백그라운드 갱신 시작
백그라운드 태스크: 새 캐시 다운로드 -> 체크섬 검증 -> 임시 파일에 쓰기 -> 원자적 rename
```

갱신은 메인 쿼리와 동시 실행되지만 결과 출력이나 프로세스 종료를 절대 지연시키지 않습니다. 갱신 완료 전에 프로세스가 종료되면 갱신은 중단되고 다음 실행에서 재시도됩니다.

---

## 8. 실시간 DNS 조회 (4글자 이상)

### 8.1 DNS-over-HTTPS 프로바이더

- **주:** Cloudflare DoH (`https://cloudflare-dns.com/dns-query`)
- **대체:** Google DoH (`https://dns.google/resolve`)

### 8.2 쿼리 포맷

```
GET https://cloudflare-dns.com/dns-query?name=example.com&type=NS
Accept: application/dns-json
```

응답 (JSON):
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

### 8.3 가용성 판단

```
DNS 응답 상태:
  - NXDOMAIN (Status: 3)  -> available = true
  - NOERROR  (Status: 0)  -> available = false (도메인 존재)
  - SERVFAIL (Status: 2)  -> 대체 프로바이더로 재시도
  - 기타                   -> 대체 프로바이더로 재시도
```

**단순화:** NXDOMAIN = 사용 가능, 나머지 = 사용 불가. 와일드카드/파킹 IP 감지 없음.

빠른 대량 탐색을 위한 의도적 휴리스틱이며, 레지스트라 측 구매 가능 여부와 정확히 일치하지 않을 수 있습니다.

### 8.4 HTTP 클라이언트 설정

```rust
// reqwest 클라이언트 설정
let client = reqwest::Client::builder()
    .connect_timeout(Duration::from_secs(5))
    .timeout(Duration::from_secs(10))
    .user_agent(format!("domaingrep/{}", env!("CARGO_PKG_VERSION")))
    .http2_prior_knowledge()  // DoH 서버는 HTTP/2 지원
    .pool_max_idle_per_host(50)
    .build()?;
```

| 설정 | 값 | 이유 |
|---|---|---|
| `connect_timeout` | 5초 | 도달 불가 서버에 빠른 실패 |
| `timeout` | 10초 | 응답 포함 최대 총 요청 시간 |
| `User-Agent` | `domaingrep/{version}` | 트래픽 식별, 좋은 네티즌 행동 |
| HTTP 버전 | HTTP/2 | 멀티플렉싱으로 연결 오버헤드 감소 |
| 풀 유휴 | 호스트당 50 | 병렬 요청 간 연결 재사용 |

### 8.5 동시성 모델

```rust
// 적응형 동시성 + 백오프
struct DnsResolver {
    primary: CloudflareDoH,
    fallback: GoogleDoH,
    semaphore: Semaphore,       // 동시 요청 제한
    initial_concurrency: usize, // 100
}
```

1. `tokio::Semaphore`로 100개 동시 요청부터 시작
2. 주 프로바이더에서 HTTP 429 (속도 제한): 해당 요청을 대체 프로바이더로 전환
3. 둘 다 실패: 해당 TLD 건너뛰기, 실패로 카운트
4. 모든 TLD를 병렬 쿼리 (`--limit`이 있어도 조기 종료 없음)
5. Domain hack 결과도 동일 로직: SLD 길이가 소스 결정 (1-3글자 -> 캐시, 4글자 이상 -> DNS)

### 8.6 TLD별 요청 흐름

```
1. 세마포어 허가 획득
2. Cloudflare에 DoH 쿼리 전송
3. 성공: 결과 반환
4. 실패 (429/타임아웃/에러):
   a. Google DNS에 DoH 쿼리 전송 (대체)
   b. 성공: 결과 반환
   c. 실패: 건너뛰기로 마킹
5. 세마포어 허가 반환
```

### 8.7 NS 레코드 쿼리

DNS 쿼리 타입은 `NS` (A가 아님):

- 등록된 도메인은 일반적으로 NS 레코드로 위임됨
- 미등록 도메인은 NXDOMAIN 반환
- A/AAAA 레코드가 없는 도메인의 false negative 방지

---

## 9. 출력 포맷

### 9.1 일반 텍스트 (기본)

```
$ domaingrep bunsh

bun.sh            # domain hack (상단)
bunsh.io
bunsh.co
bunsh.to
bunsh.com
bunsh.dev
bunsh.app
```

`--all` 사용 시:
```
$ domaingrep bunsh --all

  bun.sh            # domain hack, 사용 가능
x bunsh.io
  bunsh.co
x bunsh.to
x bunsh.com
  bunsh.dev
  bunsh.app
```

### 9.2 심볼 및 색상

| 상태 | 심볼 | 색상 |
|---|---|---|
| 사용 가능 | ` ` (공백, 2글자 들여쓰기) | 기본/초록 |
| 사용 불가 | `x` (1글자 + 공백) | 흐림/회색 |

- 색상은 ANSI escape 코드
- `isatty()`로 자동 감지: 파이프나 리다이렉트 시 비활성화
- `--color=always` 또는 `--color=never`로 오버라이드

`--all`이 **없으면** 사용 가능 도메인만 표시 (심볼 없이 도메인 이름만):

```
$ domaingrep bunsh

bun.sh
bunsh.co
bunsh.dev
bunsh.app
```

### 9.3 JSON 출력 (--json)

NDJSON 포맷 (줄당 하나의 JSON 객체):

```json
{"domain":"bun.sh","available":true,"kind":"hack","method":"cache"}
{"domain":"bunsh.io","available":true,"kind":"regular","method":"dns"}
{"domain":"bunsh.com","available":false,"kind":"regular","method":"dns"}
```

- 필드: `domain`, `available`, `kind` (`hack` 또는 `regular`), `method` (`cache` 또는 `dns`)
- `--all` 사용 시: 사용 불가 도메인 포함
- `--all` 없으면: 사용 가능 도메인만 (모두 `available: true`)

### 9.4 출력 순서

1. **Domain hack** (감지된 경우) - SLD 길이 오름차순 정렬
2. **일반 결과** - 다음 순서로 정렬:
   - 1차: TLD 길이 오름차순
   - 2차: TLD 인기도 (하드코딩 리스트)
   - 3차: 알파벳순

### 9.5 파이프 동작

stdout이 TTY가 아닐 때 (파이프):
- ANSI 색상 코드 없음
- 진행 표시 없음
- 깔끔하고 파싱 가능한 출력
- 선택된 포맷의 줄당 하나의 레코드

---

## 10. 자동 업데이트

### 10.1 GitHub 리포지토리

- **Owner/Repo:** `ysm-dev/domaingrep`
- **캐시 Release URL:** `https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.bin.gz`
- **캐시 체크섬 URL:** `https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.sha256`
- **최신 Release API:** `https://api.github.com/repos/ysm-dev/domaingrep/releases/latest`

### 10.2 CLI 버전 체크

이전 체크가 없거나 24시간 이상 경과한 실행에서 **best-effort 논블로킹** 백그라운드 체크 수행:

```
1. {cache_dir}/domaingrep/last_update_check 읽기
2. 24시간 이내: 체크 건너뛰기
3. 24시간 이상 또는 없음:
   a. async 태스크 스폰 (논블로킹)
   b. GitHub API 쿼리: GET /repos/ysm-dev/domaingrep/releases/latest
   c. 버전 태그와 현재 바이너리 버전 비교
   d. 새 버전 있고 프로세스 종료 전에 체크 완료 시: 결과 출력 후 stderr에 안내 출력
   e. 성공적 완료 시 last_update_check 타임스탬프 업데이트
```

### 10.3 업데이트 안내

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

- **stderr**에 출력 (파이프된 stdout에 간섭 없음)
- best-effort 논블로킹 (결과 출력이나 프로세스 종료를 절대 지연시키지 않음)
- 최대 24시간에 한 번

### 10.4 캐시 업데이트

캐시는 stale-while-revalidate로 업데이트 (섹션 7.6 참조). 백그라운드 갱신은 메인 쿼리와 동시에 실행되며 출력이나 종료를 절대 블로킹하지 않습니다.

---

## 11. 캐시 빌더 (GitHub Actions)

### 11.1 개요

매일 실행되는 GitHub Actions 워크플로가 모든 1-3글자 도메인의 전체 TLD DNS 조회로 bitmap 캐시를 재구축합니다.

### 11.2 규모

- ~49,284 가능 도메인 x ~1,200 TLD = ~59.1M DNS 쿼리
- 속도를 위해 병렬 matrix job으로 샤딩

이 워크로드는 규모가 크므로 아래 matrix 크기는 예시입니다. 프로덕션 빌드에서는 self-hosted runner 및/또는 명시적 대량 사용 허용이 있는 resolver 소스가 필요할 수 있습니다.

### 11.3 워크플로 설계

```yaml
# .github/workflows/cache-build.yml
name: Build Domain Cache

on:
  schedule:
    - cron: '0 2 * * *'  # 매일 UTC 2:00
  workflow_dispatch:       # 수동 트리거

jobs:
  fetch-tlds:
    runs-on: ubuntu-latest
    outputs:
      tld-groups: ${{ steps.split.outputs.groups }}
    steps:
      - uses: actions/checkout@v4
      - name: TLD 리스트 가져오기 및 필터링
        id: split
        run: |
          # tld-list.com JSON 다운로드
          # 필터: ASCII만, 공개 등록 가능, 인프라 아님
          # 병렬 처리를 위해 ~40개 TLD씩 그룹 분할
          # JSON matrix로 출력

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
      - name: 캐시 빌더 빌드
        run: cargo build --release --bin cache-builder
      - name: TLD 그룹 스캔
        run: ./target/release/cache-builder scan --tlds '${{ matrix.group }}'
      - name: 부분 bitmap 업로드
        uses: actions/upload-artifact@v4
        with:
          name: bitmap-${{ matrix.group }}
          path: partial-bitmap.bin

  merge:
    needs: scan
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: 모든 부분 bitmap 다운로드
        uses: actions/download-artifact@v4
      - name: Bitmap 병합
        run: ./target/release/cache-builder merge --output cache.bin
      - name: 압축
        run: gzip -9 cache.bin
      - name: 체크섬 생성
        run: sha256sum cache.bin.gz > cache.sha256
      - name: Release 생성/업데이트
        uses: softprops/action-gh-release@v2
        with:
          tag_name: cache-latest
          files: |
            cache.bin.gz
            cache.sha256
          prerelease: true
```

### 11.4 캐시 빌더 바이너리

위치: `src/bin/cache_builder.rs`. CLI와 같은 crate를 공유합니다.

**명령:**

```
cache-builder fetch-tlds
  # tld-list.com에서 TLD 리스트 가져오기, 필터링, JSON 출력

cache-builder scan --tlds <TLD_LIST>
  # 주어진 TLD에 대해 모든 1-3글자 도메인을 Cloudflare DoH (NS 레코드)로 스캔
  # 출력: 부분 bitmap 파일

cache-builder merge --output <PATH>
  # 모든 부분 bitmap 파일을 최종 cache.bin으로 병합
  # TLD 인덱스 테이블이 포함된 헤더 생성
```

### 11.5 DNS 쿼리 전략 (캐시 빌더)

- **프로바이더:** 프로토타입은 Cloudflare DoH JSON; 프로덕션 빌더는 전용/대량 resolver 소스 필요 가능
- **레코드 타입:** NS
- **동시성:** job당 100-200 동시 요청 (rate limit 회피를 위해 보수적으로 튜닝)
- **속도 제한:** 429 응답 시 적응형 백오프
- **재시도:** 쿼리당 3회 시도 후 사용 불가로 마킹

### 11.6 TLD 리스트 신선도 및 Probe 테스트

캐시 빌더는 매 실행마다 tld-list.com에서 TLD 리스트를 가져와 새 TLD 자동 포함 및 제거된 TLD 제거를 보장합니다.

**TLD probe 테스트 (fetch-tlds job에서):**

```
tld-list.com의 각 TLD에 대해 (ASCII/타입 필터링 후):
  1. nic.{tld}에 NS 조회
     - NS 레코드 없음 -> TLD 비활성 -> 제외
  2. xyzzy-probe-test-{random_hex}.{tld}에 NS 조회
     - NXDOMAIN -> 공개 등록 지원 -> 포함
     - NOERROR  -> 와일드카드 DNS (브랜드 TLD) -> 제외
     - SERVFAIL -> 3회 재시도 후 제외
```

이를 통해 수동 브랜드 TLD 제외 목록 유지가 불필요합니다.

### 11.7 캐시 Release 전략

캐시는 매일 덮어쓰는 고정 태그 `cache-latest`로 배포됩니다:

```
태그:    cache-latest (덮어쓰기, 버전 없음)
에셋:    cache.bin.gz, cache.sha256
URL:     https://github.com/ysm-dev/domaingrep/releases/download/cache-latest/cache.bin.gz
```

GitHub Actions 워크플로가 매일 기존 `cache-latest` release를 삭제하고 새 에셋으로 재생성합니다.

---

## 12. 배포 및 설치

### 12.1 배포 채널

| 채널 | 패키지명 | 명령 |
|---|---|---|
| Homebrew | `domaingrep` | `brew install domaingrep` |
| Shell 스크립트 | - | `curl -fsSL https://domaingrep.dev/install.sh \| sh` |
| npm | `domaingrep` | `npx domaingrep` / `npm i -g domaingrep` |
| Cargo | `domaingrep` | `cargo install domaingrep` |

### 12.2 npm 바이너리 배포

`optionalDependencies` 패턴 사용 (biome, napi-rs와 동일):

```
domaingrep                     (메인 패키지, JS 래퍼)
  @domaingrep/darwin-arm64     (macOS Apple Silicon)
  @domaingrep/darwin-x64       (macOS Intel)
  @domaingrep/linux-arm64-gnu  (Linux ARM64)
  @domaingrep/linux-arm64-musl (Linux ARM64 musl)
  @domaingrep/linux-x64-gnu    (Linux x64)
  @domaingrep/linux-x64-musl   (Linux x64 musl)
  @domaingrep/win32-x64        (Windows x64)
```

메인 `domaingrep` 패키지는 플랫폼별 바이너리를 찾아 실행하는 얇은 JS 래퍼를 포함합니다.

### 12.3 Shell 설치 스크립트

```bash
curl -fsSL https://domaingrep.dev/install.sh | sh
```

`~/.domaingrep/bin/domaingrep`에 설치하고 PATH 추가를 안내:

```
domaingrep was installed to ~/.domaingrep/bin/domaingrep
Add the following to your shell profile:
  export PATH="$HOME/.domaingrep/bin:$PATH"
```

### 12.4 크로스 컴파일 타겟

| 타겟 | OS | 아키텍처 |
|---|---|---|
| `x86_64-apple-darwin` | macOS | x64 |
| `aarch64-apple-darwin` | macOS | ARM64 |
| `x86_64-unknown-linux-musl` | Linux | x64 |
| `aarch64-unknown-linux-musl` | Linux | ARM64 |
| `x86_64-unknown-linux-gnu` | Linux | x64 |
| `x86_64-pc-windows-msvc` | Windows | x64 |

### 12.5 바이너리 최적화

`Cargo.toml`에서:

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

예상 바이너리 크기: ~3-5 MB.

---

## 13. 프로젝트 구조

### 13.1 Crate 레이아웃

```
domaingrep/
  Cargo.toml
  src/
    main.rs              # 엔트리 포인트, CLI 인수 파싱 (clap)
    lib.rs               # 라이브러리 루트, re-exports
    cli.rs               # CLI 인수 정의 (clap derive)
    input.rs             # 입력 파싱 및 검증
    hack.rs              # Domain hack 감지 (trie 기반)
    tld.rs               # TLD 리스트 관리, 필터링, 정렬
    cache.rs             # Bitmap 캐시: 로드, 조회, 다운로드, 검증
    dns.rs               # DoH 리졸버: Cloudflare + Google 대체
    output.rs            # 출력 포맷: 일반 텍스트, JSON, 색상
    update.rs            # 자동 업데이트 체크 로직
    error.rs             # 에러 타입 및 포맷
    bin/
      cache_builder.rs   # 캐시 빌더 바이너리 엔트리 포인트
  tests/
    cli.rs              # E2E CLI 테스트
    cache.rs            # 캐시 다운로드 및 조회 테스트
    dns.rs              # DNS 클라이언트 테스트 (주로 mocked)
    hack.rs             # Domain hack 감지 테스트
    output.rs           # 출력 포맷 테스트
    live_dns_smoke.rs   # 소규모 실시간 네트워크 스모크 테스트
  data/
    tld_popularity.rs    # 하드코딩된 TLD 인기도 순서
  .github/
    workflows/
      ci.yml             # PR: test + lint + build
      release.yml        # Release: cross-compile + publish
      cache-build.yml    # Daily: 캐시 재구축
```

### 13.2 모듈 책임

| 모듈 | 책임 |
|---|---|
| `cli.rs` | Clap derive 구조체, 인수 검증 |
| `input.rs` | 입력 문자열 파싱, 모드 결정 (SLD만 / SLD+TLD prefix), 문자 검증 |
| `hack.rs` | TLD 리스트로 역방향 trie 구축, domain hack 분할 찾기 |
| `tld.rs` | 캐시 헤더에서 TLD 리스트 로드, 길이/prefix로 필터링, 길이+인기도로 정렬 |
| `cache.rs` | GitHub Releases에서 캐시 다운로드, SHA-256 검증, 압축 해제, bitmap 조회, stale-while-revalidate |
| `dns.rs` | Cloudflare/Google DoH HTTP 클라이언트, NS 레코드 쿼리, 적응형 동시성, 대체 로직 |
| `output.rs` | 일반 텍스트 또는 JSON으로 결과 포맷, ANSI 색상 처리, TTY 감지 |
| `update.rs` | 최신 GitHub Release 버전 체크, 현재와 비교, 안내 출력 |
| `error.rs` | 커스텀 에러 타입, Why/Where/Fix 포맷 |

---

## 14. 테스트 전략

### 14.1 철학: 테스트 주도 개발 (TDD)

이 프로젝트는 TDD 워크플로를 따릅니다. 모든 새 모듈/기능에 대해:

1. **테스트 먼저 작성** - 구현 코드를 작성하기 전에 예상 동작을 실패하는 테스트로 정의합니다.
2. **최소한의 구현** - 테스트를 통과하는 데 필요한 최소한의 코드만 작성합니다.
3. **리팩터링** - 모든 테스트를 통과한 상태를 유지하면서 구현을 정리합니다.

이는 모든 계층에 적용됩니다: 입력 검증, bitmap 로직, DNS 조회, 출력 포맷, CLI 통합. 테스트 스위트가 시스템 동작의 살아있는 사양서입니다.

### 14.2 접근 방식

계층적 테스트 전략: 핵심 로직을 위한 결정적 unit/integration 테스트, resolver 동작을 위한 mocked HTTP 테스트, E2E 신뢰도를 위한 소규모 실시간 DNS 스모크 테스트.

### 14.3 테스트 카테고리

| 카테고리 | 설명 | 네트워크 필요 |
|---|---|---|
| 입력 검증 | 다양한 입력 파싱/검증 | 아니오 |
| Domain hack | Trie 구축, suffix 매칭 | 아니오 |
| TLD 필터링 | 길이 필터, prefix 매칭, 정렬 | 아니오 |
| Bitmap 연산 | 인덱스 계산, 비트 읽기/쓰기 | 아니오 |
| 캐시 포맷 | 직렬화/역직렬화, 체크섬 검증 | 아니오 |
| 출력 포맷 | 일반 텍스트, JSON, 색상 제거 | 아니오 |
| DNS 조회 | Mocked DoH 응답, fallback 로직, 타임아웃 처리 | 아니오 |
| 실시간 DNS 스모크 | 알려진 도메인에 대한 실제 DoH 쿼리 | 예 |
| CLI E2E | 입력에서 출력까지 전체 파이프라인 (주로 fixture 기반) | 아니오 |

### 14.4 CI 설정

```yaml
- name: Run fast test suite
  run: cargo test --all-features --lib --bins --test cli --test cache --test dns --test hack --test output

- name: Run live DNS smoke tests
  run: cargo test --test live_dns_smoke -- --ignored
```

### 14.5 테스트 데이터

안정적 가용성의 잘 알려진 도메인 사용:

```rust
// 항상 등록됨 (사용 불가)
const KNOWN_TAKEN: &[&str] = &["google.com", "github.com", "example.com"];

// 알려진 NXDOMAIN (사용 가능) - 가능성 없는 조합 사용
const KNOWN_AVAILABLE: &[&str] = &["xyzzy-test-domain-12345.com"];
```

### 14.6 커버리지 목표

문자 그대로의 100%보다는 높은 신뢰도를 목표로 합니다. 장기적으로 핵심 라이브러리 모듈에서 >=90% line coverage를 목표로 하며, `cargo-tarpaulin` 또는 `cargo-llvm-cov`로 측정합니다. 현재 CI 게이트는 자동화 테스트가 아직 다루지 않는 `src/bin/cache_builder.rs`를 제외하고, 나머지 코드에 대해 전체 line coverage >=75%를 요구합니다.

---

## 15. CI/CD 파이프라인

### 15.1 워크플로 1: CI (Pull Request)

**트리거:** main에 대한 Pull Request, main에 push

```
Job:
  1. lint:     cargo clippy --all-targets -- -D warnings
  2. fmt:      cargo fmt --check
  3. test:     cargo test --all-features --lib --bins --test cli --test cache --test dns --test hack --test output
  4. smoke:    cargo test --test live_dns_smoke -- --ignored
  5. build:    cargo build --release (컴파일 확인)
  6. coverage: cargo llvm-cov --ignore-filename-regex '(^|.*/)bin/cache_builder\.rs$' --fail-under-lines 75
```

### 15.2 워크플로 2: Release

**트리거:** Git 태그 `v*`

```
Job:
  1. 각 타겟 (6개 타겟):
     a. 크로스 컴파일: cargo build --release --target <target>
     b. 바이너리 strip
     c. 아카이브 생성 (unix: tar.gz, windows: zip)
  2. 모든 아카이브로 GitHub Release 생성
  3. crates.io에 배포: cargo publish
  4. npm 패키지 배포 (7개 플랫폼 패키지 + 메인 래퍼)
  5. Homebrew formula 업데이트 (tap repo)
  6. 새 버전으로 install.sh 생성
```

### 15.3 워크플로 3: 캐시 빌드 (매일)

**트리거:** 매일 cron (UTC 2:00), 수동 dispatch

전체 워크플로 상세는 섹션 11.3 참조.

---

## 16. 에러 핸들링

### 16.1 에러 형식

모든 에러는 stderr에 일관된 형식으로 출력됩니다:

```
error: <무엇이 잘못되었는지>
  --> <어디서/컨텍스트>
  = help: <어떻게 고치는지>
```

### 16.2 에러 시나리오

| 시나리오 | 메시지 | 종료 코드 |
|---|---|---|
| 잘못된 입력 문자 | `error: invalid character '@' in domain 'ab@c'` | 2 |
| 빈 입력 | `error: no domain provided` | 2 |
| 도메인 너무 김 | `error: domain too long (72 chars, max 63)` | 2 |
| 네트워크 없음 | `error: network request failed: connection refused` | 2 |
| 캐시 다운로드 실패 | `error: failed to download domain cache from GitHub Releases` | 2 |
| 캐시 체크섬 불일치 | `error: cache integrity check failed (SHA-256 mismatch)` | 2 |
| --limit 0 | `error: --limit must be at least 1` | 2 |
| 사용 가능 도메인 없음 | stderr: `note: no available domains found for '{input}'` | 1 |
| 부분 DNS 실패 | 결과 표시 후, stderr에: `note: N TLDs could not be checked (DNS timeout)` | 0 또는 1 |

### 16.3 부분 실패

4글자 이상 도메인 체크 중 일부 DNS 쿼리 실패 시:

1. 성공한 결과를 정상적으로 표시
2. 결과 후, stderr에 출력: `note: {N} of {total} TLDs could not be checked`
3. 종료 코드는 발견된 사용 가능 도메인 기준 (0 또는 1), 실패가 아님

---

## 17. 성능 목표

| 지표 | 목표 |
|---|---|
| 1-3글자 도메인 (웜 캐시) | < 10ms |
| 1-3글자 도메인 (콜드 캐시, 첫 다운로드, 일반 브로드밴드) | < 2초 |
| 4글자 이상 도메인 (전체 TLD, 정상 네트워크) | 일반적 < 5초 |
| 바이너리 시작 시간 | < 5ms |
| 바이너리 크기 | < 5 MB |
| 캐시 파일 크기 (gzip) | < 2 MB |
| 메모리 사용 | < 50 MB |

---

## 18. 의존성

### 18.1 Rust 크레이트 의존성

| 크레이트 | 용도 |
|---|---|
| `clap` (derive) | CLI 인수 파싱 |
| `tokio` | Async 런타임 |
| `reqwest` | HTTP 클라이언트 (DoH 쿼리, 캐시 다운로드) |
| `serde` / `serde_json` | JSON 파싱 (DoH 응답, TLD 리스트) |
| `flate2` | gzip 압축 해제 (캐시) |
| `sha2` | SHA-256 체크섬 검증 |
| `dirs` | XDG/플랫폼 캐시 디렉토리 해석 |
| `atty` / `is-terminal` | TTY 감지 (색상 출력용) |
| `anstream` / `anstyle` | ANSI 색상 출력 (clap 생태계) |

### 18.2 최소 의존성 철학

- clap/tokio/reqwest 의존성 트리에 이미 포함된 크레이트 선호
- 컴파일 시간이나 바이너리 크기를 늘리는 불필요한 의존성 회피
- DNS wire format 파싱 크레이트 불필요 (JSON DoH 사용)

---

## 부록 A: Bitmap 캐시 Wire Format

### 바이트 레벨 포맷

```
오프셋  크기  필드
0       4     매직: "DGRP" (0x44 0x47 0x52 0x50)
4       2     포맷 버전: u16 LE (현재 1)
6       8     빌드 타임스탬프: i64 LE (Unix 초)
14      2     TLD 개수: u16 LE
16      32    bitmap 데이터만의 SHA-256
48      var   TLD 인덱스 테이블: 각 TLD마다:
                1 byte: TLD 문자열 길이
                N bytes: TLD 문자열 (ASCII, 점 prefix 없음)
var     var   Bitmap 데이터:
                정렬: TLD 인덱스 (0..tld_count), 그 내 도메인 인덱스 (0..49284)
                총 비트: tld_count * 49284
                바이트 경계까지 0으로 패딩
```

### 예시

TLD 리스트 `["ai", "com", "io"]`와 도메인 "abc"에 대해:

```
TLD 인덱스: ai=0, com=1, io=2
"abc"의 도메인 인덱스:
  오프셋 = 36 (1글자) + 1296 (2글자) = 1332
  3글자 그룹 내 인덱스 = a*(37*36) + b*36 + c
    = 0*(37*36) + 1*36 + 2 = 38
  domain_index = 1332 + 38 = 1370

"abc.com"의 비트 위치: 1 * 49284 + 1370 = 50654
바이트 오프셋: 50654 / 8 = 6331
비트 오프셋:  50654 % 8 = 6
```

하이픈 포함 도메인 "a-b"에 대해:

```
domain_index:
  오프셋 = 1332
  인덱스 = 0*(37*36) + 36*36 + 1 = 1297  (하이픈은 char_val 36, 'b'는 1)
  domain_index = 1332 + 1297 = 2629
```

## 부록 B: Cloudflare DoH 응답 스키마

### 요청

```
GET https://cloudflare-dns.com/dns-query?name={domain}&type=NS
Accept: application/dns-json
```

### 응답

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
  "Answer": [],           // NXDOMAIN이면 비어있음
  "Authority": []
}
```

### 상태 코드 매핑

| DNS 상태 | 코드 | 의미 | domaingrep 해석 |
|---|---|---|---|
| NOERROR | 0 | 도메인 존재 | 사용 불가 |
| FORMERR | 1 | 형식 오류 | 재시도/건너뛰기 |
| SERVFAIL | 2 | 서버 실패 | 대체 프로바이더로 재시도 |
| NXDOMAIN | 3 | 도메인 없음 | **사용 가능** |
| NOTIMP | 4 | 미구현 | 재시도/건너뛰기 |
| REFUSED | 5 | 쿼리 거부 | 대체 프로바이더로 재시도 |

## 부록 C: 지원되는 TLD 타입

| 타입 (tld-list.com) | 포함? | 이유 |
|---|---|---|
| `ccTLD` | 예 | 국가 코드 TLD (.io, .ai, .co 등) |
| `gTLD` | 부분 | 일반 TLD, 브랜드 전용 TLD 제외 |
| `grTLD` | 예 | 일반 제한 (.biz, .name, .pro) |
| `sTLD` | 예 | 스폰서 (.aero, .asia, .museum) |
| `infrastructure` | 아니오 | .arpa만, 등록 불가 |
| IDN (punycode != null) | 아니오 | 비ASCII TLD 제외 |
