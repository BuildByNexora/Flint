# Storage Format

Flint stores limiter state in a local data directory.

```text
.flint/
  flint.aof
  flint.snapshot
  flint.lock
```

Embedded mode and shared mode use the same storage model. One writer owns the
directory at a time.

## Append-Only Log

`flint.aof` is the event log. It records every configured limit, allow, deny,
reset, and multi-limit check.

The log is append-only. Runtime state is derived by replaying events.

```text
LimitConfigured
Allow
Deny
AllowAll
Reset
```

Recent AOF records include checksum metadata. Flint verifies record integrity
during replay.

## Snapshot

`flint.snapshot` stores derived state:

- limit configs;
- token buckets;
- fixed windows;
- sliding windows;
- metrics;
- history summary;
- AOF offset included in the snapshot;
- checksum metadata.

Startup order:

1. Load snapshot if present.
2. Open `flint.aof`.
3. Replay only the AOF tail after the snapshot offset.
4. Fall back to full AOF replay if no snapshot exists.

## Compaction

Compaction writes a temporary snapshot, fsyncs it, and renames it atomically.

```text
flint.snapshot.tmp -> fsync -> rename -> flint.snapshot
```

After a successful snapshot, Flint can rotate or truncate the append-only tail.

## Crash Behavior

| Condition | Behavior |
|---|---|
| Clean shutdown | state is already persisted |
| Process killed after persisted events | replay restores state |
| Crash during final append | final truncated tail is handled deterministically |
| Corruption in the middle of the log | startup fails loudly |
| Crash during snapshot write | previous snapshot remains valid |
| Second writer on same data dir | rejected by `flint.lock` |

## Sync Modes

| Mode | Durability behavior |
|---|---|
| `always` | flush + fsync after every event |
| `batch` | write every event, fsync every N events or N ms |

`always` is the default. `batch` is intended for higher throughput workloads
that can tolerate losing the final unsynced batch after a hard crash or power
loss.

## Compatibility

Flint v0.2 can replay v0.1 AOF entries that used `per_seconds`; they are
converted to milliseconds during replay.

The storage format is versioned, but long-term compatibility should be treated
as a pre-1.0 contract until a stable 1.0 format is declared.
