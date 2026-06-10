<div align="center">

# Flint

### Persistent rate limiting, embedded first.

No Redis. No daemon. No broker.

[![License: BSD-3-Clause](https://img.shields.io/badge/License-BSD--3--Clause-blue.svg)](LICENSE)
[![CI](https://github.com/BuildByNexora/Flint/actions/workflows/ci.yml/badge.svg)](https://github.com/BuildByNexora/Flint/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/flint-limiter.svg)](https://pypi.org/project/flint-limiter/)
[![Rust](https://img.shields.io/badge/Rust-core-black.svg)](crates/flint-core)
[![Python](https://img.shields.io/badge/Python-bindings-blue.svg)](crates/flint-py)

</div>

---

## Why Flint

I was tired of adding Redis just to rate limit a local Python service.

Most rate limiters are either in-memory and reset on restart, tied to HTTP
proxies, or require a separate infrastructure service. Flint embeds inside the
Python process and persists counter state to a local append-only log.

The result is simple:

```text
Python process + .flint/ directory = durable rate limiting
```

---

## What Flint Does

Flint is an embedded rate limiter with:

- Rust core;
- Python bindings via PyO3;
- append-only AOF persistence;
- JSON snapshot and compaction;
- single-writer data directory locking;
- GIL-aware Python API;
- CLI inspect/admin commands;
- millisecond precision;
- metrics counters;
- Python decorator API;
- token bucket algorithm;
- sliding window log algorithm;
- fixed window counter algorithm;
- crash recovery from local files;
- storage doctor checks;
- v0.1 AOF compatibility for `per_seconds` entries;
- no Redis, daemon, broker, or cloud dependency.

---

## Comparison

| Solution | Persistent | No Redis | Embedded | Observable |
|---|---|---|---|---|
| Flint | Yes | Yes | Yes | Yes |
| SlowAPI | No | Yes | Yes | No |
| redis-py + limits | No | No | No | No |
| nginx rate limit | No | Yes | No | No |

Flint's unique advantage is the combination of persistent counters and zero
external infrastructure.

---

## Install

```bash
pip install flint-limiter
```

The Python module is:

```python
import flint
```

---

## Quickstart

```python
import flint

limiter = flint.Limiter(data_dir=".flint")

limiter.limit(
    "api:user-42",
    rate=100,
    per="1m",
    algorithm="token_bucket",
)

if limiter.allow("api:user-42"):
    process_request()
```

With context:

```python
result = limiter.check("api:user-42")

print(result.allowed)
print(result.remaining)
print(result.reset_at)
```

Millisecond precision:

```python
limiter.limit("burst:login", rate=1, per="250ms")
```

Decorator:

```python
@limiter.rate_limit("email:send", rate=10, per="1m")
def send_email():
    ...
```

If the limit is exceeded, Flint raises:

```python
flint.RateLimitExceeded
```

---

## CLI

```bash
flint limit add "api:user-42" --rate 100 --per 1m --algorithm token_bucket
flint limit list
flint limit status "api:user-42"
flint limit reset "api:user-42"
flint limit history "api:user-42"
flint limit top --by denied --limit 20
flint log compact
flint doctor
```

Use a custom data directory:

```bash
flint --data-dir /var/lib/myapp/flint limit status "api:user-42"
```

---

## Algorithms

| Algorithm | Use case |
|---|---|
| `token_bucket` | Smooth rate limiting with bursts |
| `sliding_window_log` | Precise rolling-window limits |
| `fixed_window_counter` | Simple high-throughput window counters |

---

## Storage

Flint stores state under `data_dir`:

```text
.flint/
  flint.aof
  flint.snapshot
  flint.lock
```

The AOF records durable events:

```text
LIMIT_CONFIGURED
ALLOW
DENY
RESET
```

On restart, Flint loads `flint.snapshot` when present, replays the AOF tail, and
restores counters. A crash-truncated final line is ignored deterministically;
corruption in the middle of the log fails loudly.

`flint doctor` validates the local storage files and reports the number of
limits, history events, AOF bytes, and whether a snapshot is present.

Flint v0.2 uses millisecond precision internally. Older v0.1 AOF entries that
stored `per_seconds` are migrated during replay by converting seconds to
milliseconds.

Metrics exposed by `status()` and `list()`:

```text
total_allowed
total_denied
last_allowed_at
last_denied_at
last_reset_at
remaining
reset_at
```

---

## What Flint Replaces

Flint replaces:

- Redis-backed rate limiting libraries;
- in-memory Python limiters that reset on restart;
- nginx-only HTTP rate limiting;
- custom database counters;
- hand-written local counters with no history.

The unique property is persistent rate limiting without Redis.

---

## Reliability Checks

The current test suite covers the core failure paths for an embedded persistent
limiter:

- exclusive data directory locking;
- concurrent checks on the same key;
- 10,000 configured limits;
- snapshot and compaction preserving status and metrics;
- recovery from append-only log;
- deterministic rejection of corrupted middle log records;
- v0.1 `per_seconds` log migration to v0.2 `per_millis`;
- Python decorator allowed/denied behavior;
- `RateLimitExceeded` metadata;
- CLI `compact`, `doctor`, and `top`.

---

## Build And Test

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Python:

```bash
python3 -m venv .venv
.venv/bin/pip install -U pip maturin pytest
.venv/bin/maturin develop
.venv/bin/python -m pytest -q tests/python
```

---

## License

BSD 3-Clause.
