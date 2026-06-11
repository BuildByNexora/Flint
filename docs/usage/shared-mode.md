# Shared Mode

Flint embedded mode writes directly to a local `.flint/` directory. That is the
simplest setup and is ideal for one process.

Shared mode starts a local HTTP server that owns the data directory as the
single writer. Other processes use the HTTP API or the Python `SharedLimiter`
client.

```text
process A ─┐
process B ─┼── HTTP ── Flint server ── .flint/
process C ─┘
```

No Redis. No broker. One Flint writer.

## Start The Server

```bash
flint --data-dir .flint-shared server start \
  --bind 127.0.0.1:7878 \
  --token dev-secret
```

For high-throughput workloads, enable batch fsync:

```bash
flint --data-dir .flint-shared server start \
  --bind 127.0.0.1:7878 \
  --token dev-secret \
  --sync batch \
  --flush-every-ms 100 \
  --flush-every-events 100 \
  --max-blocking 128
```

`sync=always` is the default and fsyncs after every event. `sync=batch` writes
every event but fsyncs in batches, so a hard crash can lose the last unsynced
batch. The server also exposes `/v1/log/flush` for explicit fsync boundaries.

## Security Defaults

Flint refuses to bind shared mode to a non-loopback address without a token.

This works:

```bash
flint server start --bind 127.0.0.1:7878
```

This is rejected:

```bash
flint server start --bind 0.0.0.0:7878
```

This works:

```bash
flint server start --bind 0.0.0.0:7878 --token "$FLINT_SERVER_TOKEN"
```

Every authenticated request uses:

```text
Authorization: Bearer <token>
```

For production-style deployments, bind on a private interface and terminate
TLS/mTLS with a reverse proxy or service mesh.

## Python Client

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

With context:

```python
result = limiter.check("api:user-42", cost=5)

print(result.allowed)
print(result.remaining)
print(result.reset_at)
```

Atomic multi-limit checks:

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

## HTTP API

| Endpoint | Method | Purpose |
|---|---|---|
| `/v1/health` | `GET` | health check |
| `/v1/limits` | `GET` | list limits |
| `/v1/limits` | `POST` | configure a limit |
| `/v1/limits/{key}` | `GET` | limit status |
| `/v1/check` | `POST` | check and consume one limit |
| `/v1/check-all` | `POST` | atomic multi-limit check |
| `/v1/reset` | `POST` | reset a limit |
| `/v1/log/flush` | `POST` | force pending batch writes to disk |
| `/v1/log/compact` | `POST` | compact AOF into snapshot |
| `/v1/doctor` | `GET` | storage/runtime health |

Configure a limit:

```bash
curl -X POST http://127.0.0.1:7878/v1/limits \
  -H "Authorization: Bearer dev-secret" \
  -H "Content-Type: application/json" \
  -d '{"key":"api:user-42","rate":100,"per":"1m","algorithm":"token_bucket"}'
```

Check a limit:

```bash
curl -X POST http://127.0.0.1:7878/v1/check \
  -H "Authorization: Bearer dev-secret" \
  -H "Content-Type: application/json" \
  -d '{"key":"api:user-42","cost":1}'
```

## When To Use Shared Mode

Use shared mode when:

- a FastAPI app runs multiple worker processes;
- a web process and a background worker need the same quota state;
- several local services need a shared limiter without Redis;
- direct access to the same `.flint/` directory would cause lock conflicts.

Use embedded mode when:

- one process owns the limiter;
- the simplest deployment is more important than cross-process sharing;
- no HTTP server is needed.
