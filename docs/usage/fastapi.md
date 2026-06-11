# FastAPI Usage

Flint ships a FastAPI/Starlette middleware as an optional extra:

```bash
pip install "flint-limiter[fastapi]"
```

The middleware uses the same embedded Rust engine as the normal Python API.
Counters are stored in `.flint/`, survive process restarts, and do not require
Redis, a broker, or a separate rate-limit service.

## Basic Route Limit

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

@app.get("/api")
def api():
    return {"ok": True}
```

When the limit is exceeded, Flint returns:

```text
HTTP 429
{"detail": "rate limit exceeded"}
```

Response headers:

```text
X-RateLimit-Limit
X-RateLimit-Remaining
X-RateLimit-Reset
Retry-After
```

## Dynamic Keys

Use `key_func` when limits depend on request data.

Per IP:

```python
app.add_middleware(
    FlintRateLimitMiddleware,
    limiter=limiter,
    key_func=lambda request: f"ip:{request.client.host}",
    rate=100,
    per="1m",
)
```

Per authenticated user:

```python
def user_key(request):
    user_id = request.headers.get("x-user-id", "anonymous")
    return f"user:{user_id}"

app.add_middleware(
    FlintRateLimitMiddleware,
    limiter=limiter,
    key_func=user_key,
    rate=1000,
    per="1h",
)
```

## Weighted Cost

Expensive endpoints can consume more quota per request.

```python
app.add_middleware(
    FlintRateLimitMiddleware,
    limiter=limiter,
    key_func=lambda request: f"user:{request.headers['x-user-id']}",
    rate=10_000,
    per="1h",
    cost=lambda request: int(request.headers.get("x-cost", "1")),
)
```

This is useful for AI APIs, billing-sensitive routes, exports, reports, or any
endpoint where one request can be more expensive than another.

## Exempt Paths

Health checks and docs routes can be excluded.

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

## Lazy Configuration

If `rate` and `per` are provided, the middleware creates limits the first time a
new dynamic key appears.

```python
app.add_middleware(
    FlintRateLimitMiddleware,
    limiter=limiter,
    key_func=lambda request: f"ip:{request.client.host}",
    rate=20,
    per="10s",
)
```

If `rate` and `per` are not provided, the limit must already exist:

```python
limiter.limit("route:/checkout", rate=30, per="1m")

app.add_middleware(
    FlintRateLimitMiddleware,
    limiter=limiter,
    key="route:/checkout",
)
```

## Multiple Uvicorn Workers

Embedded mode uses a single-writer file lock. If you run several worker
processes that all need the same counters, use shared mode instead:

```bash
flint --data-dir .flint-shared server start \
  --bind 127.0.0.1:7878 \
  --token dev-secret
```

Then connect the app to the server:

```python
import flint

limiter = flint.SharedLimiter("http://127.0.0.1:7878", token="dev-secret")
```

Use embedded mode for one process. Use shared mode when multiple processes need
one shared quota state.

