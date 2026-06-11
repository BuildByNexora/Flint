# CLI Usage

The `flint` CLI can configure, inspect, reset, compact, and serve limits.

Use a custom data directory with `--data-dir`:

```bash
flint --data-dir .flint-prod limit list
```

## Limits

Configure a limit:

```bash
flint limit add "api:user-42" --rate 100 --per 1m --algorithm token_bucket
```

Check and consume quota:

```bash
flint limit check "api:user-42"
flint limit check "api:user-42" --cost 5
```

Inspect status:

```bash
flint limit status "api:user-42"
```

List all limits:

```bash
flint limit list
```

Reset a limit:

```bash
flint limit reset "api:user-42"
```

Show history:

```bash
flint limit history "api:user-42"
```

Show busiest limits:

```bash
flint limit top --by denied --limit 20
flint limit top --by allowed --limit 20
```

## Atomic Multi-Limit Checks

```bash
flint limit check-all user:42 org:acme route:/v1/chat
```

With costs:

```bash
flint limit check-all user:42 org:acme route:/v1/chat --costs 1,10,1
```

## Storage Admin

Compact AOF into a snapshot:

```bash
flint log compact
```

Run storage health checks:

```bash
flint doctor
```

## Shared Server

Start a local shared server:

```bash
flint --data-dir .flint-shared server start \
  --bind 127.0.0.1:7878 \
  --token dev-secret
```

Start with batch fsync:

```bash
flint --data-dir .flint-shared server start \
  --bind 127.0.0.1:7878 \
  --token dev-secret \
  --sync batch \
  --flush-every-ms 100 \
  --flush-every-events 100 \
  --max-blocking 128
```

Shared mode exposes the HTTP API documented in
[Shared Mode](shared-mode.md).
