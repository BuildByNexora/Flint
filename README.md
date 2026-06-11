<div align="center">

# Flint

### Persistent rate limiting, embedded first.

No Redis. No broker. No daemon required for embedded mode.

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
- AOF and snapshot checksum validation;
- JSON snapshot and compaction;
- single-writer data directory locking;
- GIL-aware Python API;
- CLI inspect/admin commands;
- millisecond precision;
- metrics counters;
- Python decorator API;
- FastAPI middleware;
- Prometheus metrics export;
- shared HTTP server mode;
- Python `SharedLimiter` client;
- atomic multi-limit checks with `allow_all()` / `check_all()`;
- token bucket algorithm;
- sliding window log algorithm;
- fixed window counter algorithm;
- crash recovery from local files;
- storage doctor checks;
- v0.1 AOF compatibility for `per_seconds` entries;
- no Redis, broker, or cloud dependency.
- no daemon required for embedded mode.

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
result = limiter.check("api:user-42", cost=1)

print(result.allowed)
print(result.cost)
print(result.remaining)
print(result.reset_at)
```

Cost-based checks:

```python
# A normal request costs 1 unit.
limiter.check("api:user-42")

# Expensive work can consume more units from the same limit.
result = limiter.check("ai:user-42", cost=250)

if result.allowed:
    run_expensive_model_call()
```

A single check cost must be greater than zero and cannot exceed the configured
rate capacity for that limit.

Atomic multi-limit checks:

```python
result = limiter.check_all([
    "user:42",
    {"key": "org:acme", "cost": 10},
    ("endpoint:/v1/chat", 1),
])

if result["allowed"]:
    process_request()
else:
    print("blocked by", result["denied_key"])
```

`check_all()` is all-or-nothing. If any limit denies the request, Flint records
the denied limit but does not consume quota from the other limits. `allow_all()`
returns only the boolean decision:

```python
if limiter.allow_all(["user:42", "org:acme", "endpoint:/v1/chat"]):
    process_request()
```

Millisecond precision:

```python
limiter.limit("burst:login", rate=1, per="250ms")
```

Decorator:

```python
@limiter.rate_limit("email:send", rate=10, per="1m", cost=1)
def send_email():
    ...
```

If the limit is exceeded, Flint raises:

```python
flint.RateLimitExceeded
```

---

## FastAPI Middleware

Install the optional FastAPI extra:

```bash
pip install "flint-limiter[fastapi]"
```

Static route limit:

```python
from fastapi import FastAPI
import flint
from flint.fastapi import FlintRateLimitMiddleware

limiter = flint.Limiter(data_dir=".flint")
limiter.limit("route:/api", rate=100, per="1m")

app = FastAPI()
app.add_middleware(
    FlintRateLimitMiddleware,
    limiter=limiter,
    key="route:/api",
)
```

Dynamic per-client limit with lazy configuration:

```python
app.add_middleware(
    FlintRateLimitMiddleware,
    limiter=limiter,
    key_func=lambda request: f"ip:{request.client.host}",
    rate=100,
    per="1m",
    exempt_paths={"/health", "/docs", "/openapi.json"},
)
```

Blocked requests return:

```text
HTTP 429
{"detail": "rate limit exceeded"}
```

With headers:

```text
X-RateLimit-Limit
X-RateLimit-Remaining
X-RateLimit-Reset
Retry-After
```

The middleware uses the same embedded persistent engine: no Redis, no daemon, no
broker. Counters are stored in `.flint/` and survive process restarts.

---

## Shared Mode

Embedded mode is the default: one Python process owns `.flint/` directly.

Shared mode runs one Flint server as the single writer, then lets multiple
processes, workers, or services share the same persistent limits over HTTP.

Start the server:

```bash
flint --data-dir .flint-shared server start \
  --bind 127.0.0.1:7878 \
  --token dev-secret \
  --max-blocking 128
```

Use it from Python:

```python
import flint

limiter = flint.SharedLimiter(
    "http://127.0.0.1:7878",
    token="dev-secret",
    timeout=10.0,
)

limiter.limit("api:user-42", rate=100, per="1m")

if limiter.allow("api:user-42"):
    process_request()
```

Shared mode is useful when:

- a FastAPI app has multiple worker processes;
- several local services need the same quota;
- a CLI, background worker, and web process must inspect the same counters;
- you want persistent rate limiting without giving every process write access
  to the same `.flint/` directory.

HTTP API:

| Endpoint | Method | Purpose |
|---|---|---|
| `/v1/health` | `GET` | health check |
| `/v1/limits` | `GET` | list limits |
| `/v1/limits` | `POST` | configure a limit |
| `/v1/limits/{key}` | `GET` | limit status |
| `/v1/check` | `POST` | check/consume one limit |
| `/v1/check-all` | `POST` | atomic multi-limit check |
| `/v1/reset` | `POST` | reset a limit |
| `/v1/log/compact` | `POST` | compact AOF into snapshot |
| `/v1/doctor` | `GET` | storage/runtime health |

When `--token` is set, every request must include:

```text
Authorization: Bearer <token>
```

Flint refuses to bind the shared server to a non-loopback address such as
`0.0.0.0` unless a token is configured. Storage operations run on bounded
blocking workers controlled by `--max-blocking`, so persistent writes do not
block the async HTTP runtime.

Shared mode keeps Flint's core model simple: one writer owns the data directory;
other processes use the server API instead of opening the same files directly.

---

## Prometheus Metrics

Flint can export limiter state in Prometheus text format:

```python
import flint

limiter = flint.Limiter(data_dir=".flint")

metrics_text = flint.prometheus_metrics(limiter)
```

FastAPI endpoint:

```python
from fastapi import FastAPI
import flint

limiter = flint.Limiter(data_dir=".flint")
app = FastAPI()

flint.add_prometheus_route(app, limiter, path="/metrics")
```

For high-cardinality keys such as users, IPs, API keys, or tenant IDs, avoid
exporting the raw key as a Prometheus label:

```python
metrics_text = flint.prometheus_metrics(
    limiter,
    include_key_label=False,
)

flint.add_prometheus_route(
    app,
    limiter,
    path="/metrics",
    include_key_label=False,
)
```

Or bucket/redact keys before export:

```python
metrics_text = flint.prometheus_metrics(
    limiter,
    key_label_func=lambda key: key.split(":")[0],
)
```

Example output:

```text
# HELP flint_requests_allowed_total Total allowed checks.
# TYPE flint_requests_allowed_total counter
flint_requests_allowed_total{key="route:/api",algorithm="token_bucket"} 42
flint_requests_denied_total{key="route:/api",algorithm="token_bucket"} 3
flint_limit_remaining{key="route:/api",algorithm="token_bucket"} 58
```

Exported metrics include:

```text
flint_limit_info
flint_limit_rate
flint_limit_per_millis
flint_limit_remaining
flint_limit_reset_at_seconds
flint_requests_allowed_total
flint_requests_denied_total
flint_request_cost_allowed_total
flint_request_cost_denied_total
```

---

## CLI

```bash
flint limit add "api:user-42" --rate 100 --per 1m --algorithm token_bucket
flint limit list
flint limit status "api:user-42"
flint limit check "api:user-42" --cost 5
flint limit check-all "user:42" "org:acme" --cost org:acme=10
flint limit reset "api:user-42"
flint limit history "api:user-42"
flint limit top --by denied --limit 20
flint log compact
flint doctor
flint server start --bind 127.0.0.1:7878 --token dev-secret
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
ALLOW_ALL
DENY
RESET
```

On restart, Flint loads `flint.snapshot` when present, replays the AOF tail, and
restores counters. A crash-truncated final line is ignored deterministically;
corruption in the middle of the log fails loudly.

New AOF records include a SHA-256 checksum of the stored event payload. New
snapshots are written inside a checksum envelope. Older v0.1/v0.2 files without
checksums remain readable, while checksum mismatches fail startup loudly instead
of silently accepting tampered state.

`flint doctor` validates the local storage files and reports the number of
limits, history events, AOF bytes, and whether a snapshot is present.

Flint v0.2 uses millisecond precision internally. Older v0.1 AOF entries that
stored `per_seconds` are migrated during replay by converting seconds to
milliseconds.

Metrics exposed by `status()` and `list()`:

```text
total_allowed
total_denied
total_allowed_cost
total_denied_cost
last_allowed_at
last_denied_at
last_reset_at
remaining
reset_at
```

`cost` is included in check results and rate-limit exceptions.

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
- checksum rejection for tampered AOF records and snapshots;
- v0.1 `per_seconds` log migration to v0.2 `per_millis`;
- cost-based checks across token bucket, fixed window, and sliding window;
- atomic multi-limit checks with no partial quota consumption;
- FastAPI middleware static keys, dynamic keys, lazy config, weighted cost, and exempt paths;
- Prometheus text export and FastAPI `/metrics` route;
- Python decorator allowed/denied behavior;
- `RateLimitExceeded` metadata;
- CLI `compact`, `doctor`, and `top`.
- Criterion benchmarks for hot path checks, many keys, AOF replay, and compaction.

---

## Benchmarks

Flint includes Criterion benchmarks for the core Rust engine:

```bash
cargo bench -p flint-core --bench limiter
```

The benchmark suite covers:

- token bucket, sliding window, and fixed window check hot paths;
- cost-based checks;
- atomic `check_all()` across multiple limits;
- configuring 1,000 and 10,000 keys;
- reopening from AOF with 1,000 and 10,000 events;
- compacting AOF into a snapshot.

For a quick smoke run:

```bash
cargo bench -p flint-core --bench limiter -- --quick
```

Latest local quick run:

| Benchmark | Result |
|---|---:|
| token bucket persistent check | ~560 us |
| sliding window persistent check | ~570 us |
| fixed window persistent check | ~600 us |
| cost-based token bucket check | ~620 us |
| `check_all()` over 3 limits | ~590 us |
| configure 1,000 keys | ~562 ms |
| configure 10,000 keys | ~5.5 s |
| reopen from 1,000 AOF events | ~5 ms |
| reopen from 10,000 AOF events | ~49 ms |
| compact 1,000 AOF events | ~39 ms |
| compact 10,000 AOF events | ~415 ms |

These numbers are from a local quick run and are mainly useful as a regression
baseline. Full Criterion runs should be used when comparing releases or storage
changes.

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
