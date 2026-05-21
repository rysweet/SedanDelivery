# Gremlin Text Parser: Quote and Escape Handling

The Gremlin text parser correctly handles single-quoted (`'...'`) and
double-quoted (`"..."`) string literals, including escaped quotes and
backslashes, throughout all tokenization phases. This document describes the
quoting rules, the internal mechanisms that enforce them, and how to write
Gremlin queries containing special characters.

## Quoting Rules

### Supported String Delimiters

Both single quotes and double quotes delimit string literals:

```groovy
g.V().has("name", "Alice")     // double-quoted
g.V().has('name', 'Alice')     // single-quoted
g.V().has("name", 'Alice')     // mixed — each argument independent
```

A string opened with `"` must be closed with `"`, and likewise for `'`.
Quote characters of the *other* style are ordinary characters inside a string:

```groovy
g.V().has("name", "O'Malley")        // apostrophe inside double quotes
g.V().has('label', 'say "hello"')    // double quotes inside single quotes
```

### Escape Sequences

Inside a string literal, the backslash (`\`) escapes the next character:

| Sequence | Meaning |
|----------|---------|
| `\"` | Literal double-quote inside a double-quoted string |
| `\'` | Literal single-quote inside a single-quoted string |
| `\\` | Literal backslash |

Examples:

```groovy
// Escaped double quote inside double-quoted string
g.V().has("name", "say \"hello\"")

// Escaped single quote inside single-quoted string
g.V().has('name', 'O\'Malley')

// Escaped backslash — the string value ends with a literal backslash
g.V().has("path", "C:\\Users\\")

// Escaped backslash before closing quote — the \" is NOT an escape
// because the preceding \\ consumed the backslash
g.V().has("path", "trailing\\")   // value: trailing\
```

### Structural Characters Inside Strings

Parentheses, dots, commas, and predicate keywords (`.and(`, `.or(`) that
appear inside string literals are treated as ordinary string content — they
do not affect parenthesis balancing, step splitting, or predicate chain
detection:

```groovy
// Parentheses inside a string do not break paren-matching
g.V().has("desc", "count()")

// Dots inside a string do not split steps
g.V().has("url", "www.example.com")

// .and( / .or( inside a string are not predicate chain operators
g.V().has("query", "P.eq(1).and(P.gt(0))")

// Commas inside a string do not split arguments
g.V().has("csv", "a,b,c")
```

## How It Works

### `update_string_state()`

All tokenization functions share a single, canonical string-state tracker:

```rust
fn update_string_state(
    ch: char,
    string_char: &mut Option<char>,  // None = outside string, Some(q) = inside string delimited by q
    escape: &mut bool,               // true = previous char was unescaped backslash
) -> bool;
```

**State transitions:**

1. If `escape` is `true`, the current character is consumed (regardless of
   what it is) and `escape` is cleared. Returns `true`.
2. If inside a string (`string_char` is `Some(q)`):
   - `\` sets `escape = true`. Returns `true`.
   - The matching quote `q` closes the string (`string_char = None`). Returns `true`.
   - Any other character is string content. Returns `true`.
3. If outside a string:
   - `"` or `'` opens a string (`string_char = Some(ch)`). Returns `true`.
   - Any other character is structural. Returns `false`.

When `update_string_state()` returns `true`, the caller skips all structural
processing (paren counting, step splitting, etc.) for that character.

### Functions Using This Pattern

| Function | Purpose | Quote-aware? |
|----------|---------|:------------:|
| `skip_balanced_parens` | Skip past `(...)` blocks | ✅ via `update_string_state()` |
| `find_matching_paren` | Find closing `)` position | ✅ via `update_string_state()` |
| `split_top_level_args` | Split comma-separated args | ✅ via `update_string_state()` |
| `split_predicate_chain` | Detect `.and(`/`.or(` chains | ✅ via inline byte-level escape tracking |
| `tokenize_steps` | Split `g.V().has(...)` into steps | ✅ via `update_string_state()` |

`split_predicate_chain` uses byte-level scanning (operating on `&[u8]`) for
performance-critical byte-index arithmetic. It implements the same escape
logic inline: an `in_string: Option<u8>` tracks the opening quote byte, and
an `escape: bool` flag handles backslash escapes. This is safe because all
structural characters (`(`, `)`, `.`, `'`, `"`, `\`) are single-byte ASCII
and never collide with UTF-8 continuation bytes (0x80–0xBF).

## Security Considerations

The Gremlin text protocol accepts arbitrary user input over WebSocket.
Correct quote handling prevents **parser confusion attacks** where a crafted
input alters query structure:

```groovy
// Without proper escaping, the \" would close the string early,
// and ).drop() would execute as a separate step — deleting data.
g.V().has("key", "value\").drop()")
//                        ^^
// With correct handling, \" is an escaped quote inside the string.
// The entire "value\").drop()" is the property value.
// No .drop() step is parsed.
```

**Guarantees:**

- `\"` inside a double-quoted string is always a literal quote, never a string terminator.
- `\\\"` (escaped backslash + quote) correctly closes the string at the `"` — the `\\`
  produces a literal backslash, and the following `"` is an unescaped close-quote.
- `.and(` and `.or(` inside any quoted string are never interpreted as predicate
  chain operators.
- Parentheses inside strings never affect depth counting.

## Examples

### Property values with special characters

```groovy
// JSON-like content
g.V().has("config", "{\"key\": \"value\"}")

// Path with backslashes
g.V().has("path", "C:\\Program Files\\app")

// Query-like content that must not be parsed
g.V().has("saved_query", "g.V().has('name','test').drop()")
```

### Predicate chains with string arguments

```groovy
// The .and() here is a predicate chain operator (outside strings)
g.V().has("age", P.gte(18).and(P.lt(65)))

// The .and( inside the string is NOT a chain operator
g.V().has("formula", P.eq("x.and(y)"))

// Mixed: real chain with string containing structural chars
g.V().has("x", P.eq("a.b(c)").and(P.neq("d.e(f)")))
```

### Empty strings

```groovy
g.V().has("name", "")    // empty double-quoted
g.V().has("name", '')    // empty single-quoted
```

## Testing

Quote handling is covered by unit tests in `gremlin-core/src/text_parser/tests.rs`.
The test suite includes:

- **`find_matching_paren`**: Escaped double quotes, escaped single quotes,
  escaped backslash before close quote, strings containing parentheses,
  empty strings.
- **`split_predicate_chain`**: Strings containing `.and(`/`.or(` patterns,
  strings with dots and parentheses, mixed quote styles.
- **Integration tests**: Full `parse_traversal` round-trips with `has()`
  steps containing escaped quotes and predicate strings with structural
  characters.

Run the tests:

```bash
cargo test -p gremlin-core --lib
```

Run with clippy:

```bash
cargo clippy -p gremlin-core --lib --tests -- -D warnings
```
