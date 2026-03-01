# SedanDelivery Performance Benchmarks

## Running benchmarks

```bash
cargo bench -p sedan-bolt
```

To run a specific benchmark by name:

```bash
cargo bench -p sedan-bolt -- encode_int
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

## Baseline (to be filled after first run)

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
