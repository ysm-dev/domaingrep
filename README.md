# domaingrep

Super fast bulk domain availability search across every TLD.

```
$ domaingrep bunsh
bun.sh
bunsh.com
bunsh.net
bunsh.org
bunsh.xyz
bunsh.co
bunsh.io
bunsh.dev
...
```

One keyword. Every TLD. Results in under a second.

## Install

Run instantly without installing:

```sh
npx domaingrep abc
bunx domaingrep abc
```

Or install globally:

```sh
curl -fsSL https://domaingrep.dev/install.sh | sh   # Shell (macOS / Linux)
brew install ysm-dev/tap/domaingrep                 # Homebrew
cargo install domaingrep                            # Cargo
npm  install -g domaingrep                          # npm
bun  install -g domaingrep                          # Bun
```

## Usage

**Search all TLDs:**

```
$ domaingrep abc
abc.com
abc.net
abc.sh
abc.xyz
...
```

**Domain hack detection** -- automatically finds creative splits where
the TLD is part of the word:

```
$ domaingrep bunsh
bun.sh
bunsh.com
bunsh.io
bunsh.dev
...

$ domaingrep openai
open.ai
openai.com
openai.dev
...
```

**Filter by TLD prefix:**

```
$ domaingrep abc.c
abc.com
abc.co
abc.cc
abc.club
...
```

**Filter by TLD length:**

```
$ domaingrep abc --tld-len 2
abc.ai
abc.co
abc.io
abc.me
abc.sh
abc.so
abc.to
...
```

**Show all results** including taken domains:

```
$ domaingrep abc --all
  abc.sh
x abc.com
x abc.net
  abc.xyz
  abc.co
...
```

**JSON output** for scripting:

```
$ domaingrep bunsh --json --all
{"domain":"bun.sh","available":true,"kind":"hack","method":"cache"}
{"domain":"bunsh.com","available":false,"kind":"regular","method":"dns"}
{"domain":"bunsh.io","available":true,"kind":"regular","method":"dns"}
```

Pipe-friendly: when stdout is not a TTY, the default limit is removed
and colors are disabled.

```sh
domaingrep abc --json | jq -r 'select(.available) | .domain'
```

## Options

```
domaingrep [OPTIONS] <DOMAIN>
```

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--all` | `-a` | | Show unavailable domains too |
| `--json` | `-j` | | Output as NDJSON |
| `--tld-len <RANGE>` | `-t` | | Filter TLDs by length: `2`, `2..5`, `..3`, `4..` |
| `--limit <N>` | `-l` | `25` | Max results (`0` for unlimited, unlimited when piped) |
| `--color <WHEN>` | | `auto` | `auto`, `always`, `never` |

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | At least one available domain found |
| `1` | No available domains found |
| `2` | Invalid input or error |

<details>
<summary>Environment variables</summary>

All configuration is via environment variables. Defaults work out of the box.

| Variable | Default | Description |
|----------|---------|-------------|
| `DOMAINGREP_RESOLVERS` | Public DNS | Custom DNS resolvers (comma or space separated) |
| `DOMAINGREP_RESOLVE_CONCURRENCY` | `1000` | Max in-flight DNS queries |
| `DOMAINGREP_RESOLVE_TIMEOUT_MS` | `500` | Per-attempt timeout in ms |
| `DOMAINGREP_RESOLVE_ATTEMPTS` | `4` | Max retry attempts per domain |
| `DOMAINGREP_CACHE_DIR` | Platform cache dir | Override cache directory |
| `DOMAINGREP_DISABLE_UPDATE` | `false` | Disable background version check |

</details>

## License

MIT
