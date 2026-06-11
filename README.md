<div align="center">

# Flint

Persistent rate limiting for Python, powered by Rust.

[![License: BSD-3-Clause](https://img.shields.io/badge/License-BSD--3--Clause-blue.svg)](LICENSE)
[![CI](https://github.com/BuildByNexora/Flint/actions/workflows/ci.yml/badge.svg)](https://github.com/BuildByNexora/Flint/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/flint-limiter.svg)](https://pypi.org/project/flint-limiter/)

</div>

---

## What It Is

Flint is an embedded rate limiter.

It runs inside a Python process, stores limiter state in a local `.flint/`
directory, and does not require Redis, a broker, a daemon, or a cloud service.

Use it when you need request limits, API quotas, weighted checks, or local abuse
protection with state that survives process restarts.

---

## Install

```bash
pip install flint-limiter
```

```python
import flint
```

---

## Basic Usage

```python
import flint

limiter = flint.Limiter(data_dir=".flint")

limiter.limit("api:user-42", rate=100, per="1m")

if limiter.allow("api:user-42"):
    process_request()
```

Get details instead of only a boolean:

```python
result = limiter.check("api:user-42")

print(result.allowed)
print(result.remaining)
print(result.reset_at)
```

---

## Algorithms

```python
limiter.limit("login", rate=5, per="1m", algorithm="token_bucket")
limiter.limit("search", rate=100, per="1m", algorithm="sliding_window_log")
limiter.limit("exports", rate=10, per="1h", algorithm="fixed_window_counter")
```

Supported durations:

```text
ms, s, m, h, d
```

Example:

```python
limiter.limit("burst", rate=1, per="250ms")
```

---

## Cost-Based Checks

One request can consume more than one unit.

```python
limiter.limit("ai:user-42", rate=10_000, per="1h")

result = limiter.check("ai:user-42", cost=250)

if result.allowed:
    run_model_call()
```

---

## Atomic Multi-Limit Checks

Check several limits as one operation.

```python
result = limiter.check_all([
    "user:42",
    {"key": "org:acme", "cost": 10},
    ("route:/v1/chat", 1),
])

if result["allowed"]:
    process_request()
else:
    print("blocked by", result["denied_key"])
```

If one limit denies the request, Flint does not consume quota from the other
limits.

---

## Decorator

```python
@limiter.rate_limit("email:send", rate=10, per="1m")
def send_email():
    send()
```

When the limit is exceeded:

```python
try:
    send_email()
except flint.RateLimitExceeded as exc:
    print(exc.key, exc.remaining, exc.reset_at)
```

---

## Async Usage

The async methods run the same limiter operations in a thread executor.

```python
await limiter.alimit("api:user-42", rate=100, per="1m")
result = await limiter.acheck("api:user-42")
status = await limiter.astatus("api:user-42")
```

Available async methods:

```text
alimit, aallow, acheck, aallow_all, acheck_all, astatus, alist,
areset, ahistory, acompact, aflush, adoctor, atop
```

---

## FastAPI

Install the optional extra:

```bash
pip install "flint-limiter[fastapi]"
```

```python
from fastapi import FastAPI
import flint
from flint.fastapi import FlintRateLimitMiddleware

limiter = flint.Limiter(data_dir=".flint")

app = FastAPI()
app.add_middleware(
    FlintRateLimitMiddleware,
    limiter=limiter,
    key_func=lambda request: f"ip:{request.client.host}",
    rate=100,
    per="1m",
)
```

Exceeded requests return `HTTP 429` with rate-limit headers.

---

## Shared Mode

Embedded mode is single-writer. One process owns one `.flint/` directory.

If multiple processes need the same limiter state, start a local Flint server:

```bash
flint --data-dir .flint-shared server start \
  --bind 127.0.0.1:7878 \
  --token dev-secret
```

Client:

```python
import flint

limiter = flint.SharedLimiter("http://127.0.0.1:7878", token="dev-secret")
limiter.limit("api:user-42", rate=100, per="1m")
limiter.allow("api:user-42")
```

Shared mode is useful for multiple web workers, local services, or a CLI and app
that need the same quota state.

The shared server owns the data directory. A second shared server started with
the same `--data-dir` fails because `flint.lock` is already held.

---

## Persistence

Flint stores state locally:

```text
.flint/
  flint.aof
  flint.snapshot
  flint.lock
```

Storage behavior:

- `flint.aof` is an append-only event log.
- `flint.snapshot` stores compacted derived state.
- `flint.lock` prevents two writers on the same directory.
- shared mode is tested to reject a second server on the same data directory.
- AOF and snapshot records include integrity checks.
- Middle corruption fails loudly.
- A truncated final AOF tail is handled as a crash tail.

Sync modes:

```python
safe = flint.Limiter(data_dir=".flint", sync="always")

fast = flint.Limiter(
    data_dir=".flint-fast",
    sync="batch",
    flush_every_ms=100,
    flush_every_events=100,
)
```

`always` fsyncs every event. `batch` writes every event but fsyncs periodically.

---

## CLI

```bash
flint limit add "api:user-42" --rate 100 --per 1m
flint limit check "api:user-42"
flint limit status "api:user-42"
flint limit list
flint limit top --by denied --limit 20
flint log compact
flint doctor
```

Use a custom data directory:

```bash
flint --data-dir /var/lib/myapp/flint limit list
```

---

## Prometheus

```python
from flint.prometheus import prometheus_metrics

print(prometheus_metrics(limiter))
```

FastAPI route helper:

```python
from flint.prometheus import add_prometheus_route

add_prometheus_route(app, limiter)
```

---

## Benchmarks

Command:

```bash
cargo bench -p flint-core --bench limiter
```

Latest local full Criterion run:

| Benchmark | Result |
|---|---:|
| token bucket persistent check | ~569 us |
| sliding window persistent check | ~581 us |
| fixed window persistent check | ~580 us |
| cost-based token bucket check | ~570 us |
| `check_all()` over 3 limits | ~654 us |
| configure 1,000 keys | ~581 ms |
| configure 10,000 keys | ~5.71 s |
| reopen from 1,000 AOF events | ~5.53 ms |
| reopen from 10,000 AOF events | ~50.8 ms |
| compact 1,000 AOF events | ~33.5 ms |
| compact 10,000 AOF events | ~354 ms |

Numbers depend on hardware, filesystem, and sync mode.

---

## Documentation

- [Python Usage](docs/usage/python.md)
- [CLI Usage](docs/usage/cli.md)
- [FastAPI Usage](docs/usage/fastapi.md)
- [Shared Mode](docs/usage/shared-mode.md)
- [Security Guide](docs/usage/security.md)
- [Storage Format](docs/reference/storage-format.md)
- [Release Checklist](docs/usage/release.md)

---

## Build And Test

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

```bash
python3 -m venv .venv
.venv/bin/pip install -U pip maturin pytest
.venv/bin/maturin develop
.venv/bin/python -m pytest -q tests/python
```

---

## License

BSD 3-Clause. See [LICENSE](LICENSE).
