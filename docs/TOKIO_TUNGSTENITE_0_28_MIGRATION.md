# tokio-tungstenite 0.28 Migration

## Overview

SedanDelivery's WebSocket-based servers (`sedandelivery-bolt`, `sedandelivery-gremlin`, `sedandelivery-gremlin-server`) use `tokio-tungstenite` 0.28. This document describes the resulting API and conventions for contributors and operators.

## Dependency Versions

All WebSocket-using crates pin `tokio-tungstenite = "0.28"`:

| Crate | Manifest | Kind |
|---|---|---|
| `sedandelivery-bolt` | `sedandelivery-bolt/Cargo.toml` | dependency (already at 0.28 prior to this change) |
| `sedandelivery-gremlin` | `sedandelivery-gremlin/Cargo.toml` | dependency + dev-dependency |
| `sedandelivery-gremlin-server` | `sedandelivery-gremlin-server/Cargo.toml` | dependency |

There is no `[workspace.dependencies]` entry for `tokio-tungstenite` — pins live per-crate to keep the change minimal and localized.

## Breaking API Changes Adopted

`tokio-tungstenite` 0.28 (via `tungstenite` 0.28) introduces two changes that affect SedanDelivery code:

1. **`Message::Text` now carries `Utf8Bytes`** (a refcounted, UTF-8-validated byte buffer) instead of `String`. `Message::Binary`, `Message::Ping`, and `Message::Pong` carry `Bytes` instead of `Vec<u8>`.
2. **`Message` and `Error` are `#[non_exhaustive]`**, so all `match` expressions over them must include a catch-all arm.

## Conventions

### Constructing outbound messages

Use `.into()` from `String`, `&str`, or `Vec<u8>` at construction sites:

```rust
use tokio_tungstenite::tungstenite::Message;

// Text frames
let payload: String = serde_json::to_string(&response)?;
ws.send(Message::Text(payload.into())).await?;

// Binary frames
let frame: Vec<u8> = encode_binary(&response);
ws.send(Message::Binary(frame.into())).await?;

// Ping/Pong
ws.send(Message::Ping(vec![1, 2, 3].into())).await?;
```

### Consuming inbound text

`Message::Text(text)` yields a `Utf8Bytes`. Prefer `.as_str()` when feeding `&str`-shaped APIs (such as `serde_json::from_str`); use `.to_string()` only when ownership of an owned `String` is required. Sites that previously cloned `String` now clone `Utf8Bytes` directly — clone is O(1) (refcounted), so no special handling is needed.

```rust
use tokio_tungstenite::tungstenite::Message;

while let Some(msg) = ws.next().await {
    match msg? {
        Message::Text(text) => {
            // text: Utf8Bytes — cheap to clone (refcounted)
            let request: GremlinRequest = serde_json::from_str(text.as_str())?;
            handle(request).await?;
        }
        Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {
            // handled by the underlying stream or intentionally ignored
        }
        Message::Close(_) => break,
        _ => {
            tracing::trace!("ignoring unhandled websocket message variant");
        }
    }
}
```

### Catch-all match arms

Every `match` over `Message` (and over `tungstenite::Error`, where matched explicitly) includes a catch-all arm. Two patterns are accepted:

- **Receive loops** ignore unknown variants with a low-severity trace log:
  ```rust
  _ => { tracing::trace!("ignoring unhandled websocket message variant"); }
  ```
- **Error mapping sites** propagate unknown errors via the crate's existing error type:
  ```rust
  _ => return Err(WsError::Transport("unhandled tungstenite error".into())),
  ```

**Catch-all arms must not log frame payloads.** Only the variant kind may be logged, to avoid leaking query data or credentials.

**Never swallow unknown errors with `_ => Ok(())`.** Unknown error variants must propagate so that future `tungstenite` releases cannot silently break error-handling assumptions.

### `Utf8Bytes` import path

When the type must be named explicitly (rare — `.into()` covers most call sites), import from the re-exported path:

```rust
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Bytes;
```

### `WebSocketConfig` limits

The 0.24→0.28 bump preserves all `WebSocketConfig` fields used by SedanDelivery. Frame-size limits are configured at:

- `sedandelivery-gremlin/src/connection/mod.rs` — `max_frame_size: Some(64 * 1024)`
- `sedandelivery-gremlin-server/src/lib.rs` — `max_frame_size: Some(64 * 1024)`

`sedandelivery-bolt` does not customize `WebSocketConfig`; message-size enforcement happens in `sedandelivery-bolt/src/message/codec.rs::max_message_size`. Any future tuning of these limits should be done at those sites — do not silently accept upstream defaults.

## Updated Files

The following files were touched by the migration:

- `sedandelivery-gremlin/Cargo.toml`, `sedandelivery-gremlin-server/Cargo.toml` — version bump from 0.24 to 0.28.
- `sedandelivery-gremlin/src/connection/mod.rs` — Text construction via `.into()`, consumption via `.as_str()`, catch-all arm.
- `sedandelivery-gremlin-server/src/lib.rs` — same pattern. Sites that previously cloned `String` now clone `Utf8Bytes` (O(1), refcounted); sites that fed `&str` APIs use `.as_str()` instead.
- `sedandelivery-gremlin/tests/websocket_integration.rs` and `sedandelivery-gremlin-server/tests/ws_integration.rs` — test message construction and assertions updated for `Utf8Bytes`/`Bytes`.
- `Cargo.lock` — regenerated by cargo.

`sedandelivery-bolt` was already on 0.28; its `match` over `Message` in `sedandelivery-bolt/src/websocket/adapter.rs` already includes a catch-all arm and required no source changes. Any future contributor adding new `match` sites in bolt must follow the same convention.

## Validation

The migration is validated by the standard workspace gates:

```bash
cargo build --workspace
cargo test -p sedandelivery-bolt -p sedandelivery-gremlin -p sedandelivery-gremlin-server
cargo clippy --workspace --tests -- -D warnings
cargo audit  # check for new transitive RUSTSEC advisories introduced by the bump
```

Pre-existing CI failures unrelated to this change are tolerated per repository convention; all other checks must be green.

## Security Notes

- `Utf8Bytes` enforces UTF-8 validity at the type level for Text frames, removing a class of latent decoding errors.
- Existing `WebSocketConfig` limits (`max_message_size`, `max_frame_size`) are preserved; defaults from 0.28 are not silently accepted.
- Parse-path error handling is unchanged: malformed input still closes the connection or returns an error — no new `unwrap()` calls were added on `serde_json::from_str` or related sites.
- Refcounted `Utf8Bytes` / `Bytes` payloads do not extend sensitive-data lifetime beyond what the prior `String` / `Vec<u8>` clones did.

## Migration Cheat Sheet

| Before (0.24) | After (0.28) |
|---|---|
| `Message::Text(s)` where `s: String` | `Message::Text(s.into())` |
| `Message::Binary(v)` where `v: Vec<u8>` | `Message::Binary(v.into())` |
| `Message::Ping(vec![..])` | `Message::Ping(vec![..].into())` |
| `serde_json::from_str(&text)` (text: `String`) | `serde_json::from_str(text.as_str())` (text: `Utf8Bytes`) |
| Exhaustive `match msg { Text/Binary/Ping/Pong/Close => .. }` | Add `_ => { tracing::trace!(..); }` |
| Exhaustive `match err { .. }` on `tungstenite::Error` | Add `_ => return Err(..)` (never `_ => Ok(())`) |

> Note: `Message::Frame` exists in `tungstenite` but is not normally produced by the high-level WebSocket stream APIs; application code should let the catch-all arm absorb it rather than matching it explicitly.

## Related

- Issue: [#1794](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1794)
- Superseded PR: #870 (dependabot, closed)
- Upstream changelog: [tokio-tungstenite 0.28 release notes](https://github.com/snapview/tokio-tungstenite/releases)
