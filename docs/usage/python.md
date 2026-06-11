# Python Usage

Flint embeds a persistent rate limiter inside a Python process.

```bash
pip install flint-limiter
```

```python
import flint

limiter = flint.Limiter(data_dir=".flint")
```

The `data_dir` contains Flint's append-only log, snapshot, lock file, and local
state. Embedded mode is designed for one process owning one data directory.

## Configure A Limit

```python
limiter.limit(
    "api:user-42",
    rate=100,
    per="1m",
    algorithm="token_bucket",
)
```

Supported algorithms:

| Algorithm | Name |
|---|---|
| Token bucket | `token_bucket` |
| Sliding window log | `sliding_window_log` |
| Fixed window counter | `fixed_window_counter` |

Supported durations:

```text
ms, s, m, h, d
```

Examples:

```python
limiter.limit("short", rate=10, per="250ms")
limiter.limit("minute", rate=100, per="1m")
limiter.limit("daily", rate=10_000, per="1d")
```

## Check And Consume

```python
if limiter.allow("api:user-42"):
    process_request()
```

For detailed context:

```python
result = limiter.check("api:user-42")

print(result.allowed)
print(result.remaining)
print(result.reset_at)
print(result.algorithm)
```

## Weighted Cost

Use `cost` when requests have different weight.

```python
result = limiter.check("ai:user-42", cost=250)

if result.allowed:
    run_model_call()
```

The cost must be greater than zero and cannot exceed the configured rate
capacity for that limit.

## Atomic Multi-Limit Checks

`check_all()` is all-or-nothing. If one limit denies the request, Flint does not
consume quota from the other limits.

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

## Decorator

```python
@limiter.rate_limit("email:send", rate=10, per="1m")
def send_email():
    send()
```

When the limit is exceeded, Flint raises `flint.RateLimitExceeded`.

```python
try:
    send_email()
except flint.RateLimitExceeded as exc:
    print(exc.key)
    print(exc.remaining)
    print(exc.reset_at)
```

## Async Wrapper

The async API runs the sync limiter operation in an executor.

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

This API is intended for native `asyncio`, FastAPI, aiohttp, and other async
applications that want to call Flint directly without blocking the event loop
with file I/O. The core limiter remains the same Rust engine; the async methods
delegate each operation to Python's default thread executor.

Operational notes:

- use the normal sync API for sync applications;
- use the FastAPI middleware for route-level request limiting;
- use `acheck()` / `aallow()` when your async code needs manual decisions;
- configure the event loop's default executor if the service does very high
  limiter traffic;
- use shared mode if several worker processes need the same quota state.

## Sync Modes

```python
safe = flint.Limiter(data_dir=".flint", sync="always")

fast = flint.Limiter(
    data_dir=".flint-fast",
    sync="batch",
    flush_every_ms=100,
    flush_every_events=100,
)
```

`sync="always"` fsyncs every event and is the default. `sync="batch"` writes
every event but delays fsync for higher throughput.

Call `flush()` before important external boundaries:

```python
limiter.flush()
```

## Inspect State

```python
limiter.status("api:user-42")
limiter.list()
limiter.history("api:user-42")
limiter.top(by="denied", limit=20)
limiter.doctor()
```

## Compact Storage

```python
limiter.compact()
```

Compaction writes the current derived state to `flint.snapshot` and starts a new
append-only tail.
