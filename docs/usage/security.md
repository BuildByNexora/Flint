# Security Guide

Flint is embedded-first. Its primary security boundary is local process and file
ownership.

## Embedded Mode

Embedded mode stores state in a local data directory.

```text
.flint/
  flint.aof
  flint.snapshot
  flint.lock
```

Recommended deployment:

- place `data_dir` outside the web root;
- restrict ownership to the application user;
- use one writer per data directory;
- do not share `.flint/` over unsafe network filesystems;
- back up the data directory if limiter history is operationally important.

On Unix, Flint sets restrictive permissions for the data directory and lock file
where applicable.

## Single-Writer Lock

Flint uses `flint.lock` to prevent two embedded runtimes from writing to the same
storage at the same time.

If several processes need the same quota state, use shared mode instead of
opening the same directory directly.

## Shared Mode Authentication

Shared mode uses bearer token authentication.

```text
Authorization: Bearer <token>
```

Flint refuses to bind to a non-loopback address such as `0.0.0.0` unless a token
is configured.

## Network Deployment

Recommended production-style pattern:

```text
private clients -> TLS/mTLS proxy -> Flint shared server on private address
```

Use a reverse proxy, service mesh, or platform gateway for:

- TLS;
- mTLS;
- request logging;
- IP allowlists;
- secret injection;
- token rotation workflow.

Keep Flint bound to a private interface whenever possible.

Flint's shared server does not terminate TLS itself. That keeps the binary small
and deployment-neutral. Put TLS/mTLS, certificate rotation, public exposure,
and enterprise network policy in the reverse proxy, service mesh, load balancer,
or platform gateway in front of Flint.

Example local-only deployment:

```bash
flint --data-dir /var/lib/flint server start \
  --bind 127.0.0.1:7878 \
  --token "$FLINT_SERVER_TOKEN"
```

Example private-network deployment:

```bash
flint --data-dir /var/lib/flint server start \
  --bind 10.0.10.12:7878 \
  --token "$FLINT_SERVER_TOKEN"
```

For private-network deployment, restrict inbound traffic to trusted clients or
to the proxy/service-mesh sidecar.

## Tokens

Use high-entropy tokens from a secret manager.

Avoid:

- committing tokens to Git;
- placing tokens in public logs;
- using shared development tokens in production.

Rotate tokens by restarting the shared server with a new token or by replacing
the surrounding proxy/service-mesh secret.

During rotation, prefer a short planned restart or run the new server behind the
proxy before moving traffic. Clients should receive tokens from the same secret
manager or deployment system as the server.

## Application-Level Safety

Rate limiting is not a substitute for authentication, authorization, billing
checks, or abuse detection.

Use Flint as one layer:

- per IP;
- per user;
- per organization;
- per API route;
- per expensive operation.

For external side effects, use idempotency keys and application-level checks
where needed.
