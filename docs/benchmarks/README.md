# SedanDelivery Performance Benchmarks

## Running benchmarks

```bash
cargo bench -p sedandelivery-bolt
```

To run a specific benchmark by name:

```bash
cargo bench -p sedandelivery-bolt -- encode_int
```

HTML reports are generated in `target/criterion/` after each run.

## Benchmark suite

### PackStream encode

| Benchmark | Description |
|-----------|-------------|
| `encode_int` | Encode a single `BoltValue::Int(42)` (TinyInt path) |
| `encode_string_short` | Encode an 11-byte string ("hello world") |
| `encode_string_100` | Encode a 100-byte string |
| `encode_string_1k` | Encode a 1000-byte string |
| `encode_map_10` | Encode a map with 10 string-keyed integer entries |
| `encode_list_100` | Encode a list of 100 integers |
| `encode_structure_node` | Encode a Node-like structure (id + labels + properties) |

### PackStream decode

| Benchmark | Description |
|-----------|-------------|
| `decode_int` | Decode a pre-encoded integer |
| `decode_string_short` | Decode a pre-encoded 11-byte string |
| `decode_string_1k` | Decode a pre-encoded 1000-byte string |
| `decode_map_3` | Decode a pre-encoded 3-entry map |

### Encode/decode roundtrip

| Benchmark | Description |
|-----------|-------------|
| `roundtrip_map_mixed` | Encode then decode a map with strings, ints, floats, bools |
| `roundtrip_structure_node` | Encode then decode a Node structure |

### Message construction

| Benchmark | Description |
|-----------|-------------|
| `msg_encode_success` | Encode a SUCCESS server message with metadata |
| `msg_encode_record` | Encode a RECORD server message with 4 fields |
| `msg_encode_failure` | Encode a FAILURE server message with error details |

### Read-routing decision

| Benchmark | Description |
|-----------|-------------|
| `read_consistency_no_recent_write` | Session has no recent write; use topology-default routing |
| `read_consistency_recent_write` | Session is still inside the primary-pinned freshness window — representative inside-window probe at `1s` age |
| `read_consistency_window_expired` | Freshness window expired; return to topology-default routing — probe at `READ_AFTER_WRITE_WINDOW + 1s` |

These microbenchmarks import the public
`sedandelivery_bolt::protocol::handler::READ_AFTER_WRITE_WINDOW` constant for
the expired-window probe instead of hard-coding that boundary. The inside-window
probe remains a fixed `1s` representative age.

To run only the routing benchmarks:

```bash
cargo bench -p sedandelivery-bolt --bench bolt_benchmarks -- read_consistency
```

See [read-after-write-routing.md](../read-after-write-routing.md#read_after_write_window-and-the-routing-benchmark)
for the full design rationale.

## Routing baseline snapshot

| Benchmark | Median |
|-----------|--------|
| `read_consistency_no_recent_write` | `10.613 ns` |
| `read_consistency_recent_write` | `2.7804 ns` |
| `read_consistency_window_expired` | `2.7863 ns` |

These numbers are intentionally tiny: the routing policy check is nanoseconds,
so the real user-visible overhead shows up in `tests/loadtest`, not here.

## PackStream/message baseline (to be filled separately)

| Benchmark | Target | Measured |
|-----------|--------|----------|
| encode_int | - | TBD |
| encode_string_short | - | TBD |
| encode_string_100 | - | TBD |
| encode_string_1k | - | TBD |
| encode_map_10 | - | TBD |
| encode_list_100 | - | TBD |
| encode_structure_node | - | TBD |
| decode_int | - | TBD |
| decode_string_short | - | TBD |
| decode_string_1k | - | TBD |
| decode_map_3 | - | TBD |
| roundtrip_map_mixed | - | TBD |
| roundtrip_structure_node | - | TBD |
| msg_encode_success | - | TBD |
| msg_encode_record | - | TBD |
| msg_encode_failure | - | TBD |
