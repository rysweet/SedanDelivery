# embedded_quickstart

Tiny runnable demo that uses **all three** SedanDelivery embedded engines —
`sedan-cypher-embedded`, `sedan-gql-embedded`, and `sedan-gremlin-embedded` —
against a fresh in-memory store.

```bash
cargo run -p embedded_quickstart
```

Expected output (abridged):

```
== sedan-cypher-embedded ==
  Bob (25)
  Alice (30)
  Carol (41)

== sedan-gql-embedded ==
  Bob (25)
  Alice (30)
  Carol (41)

== sedan-gremlin-embedded ==
  Bob
  Alice
  Carol

QUICKSTART_OK
```

The full guide is in [`docs/EMBEDDED_GUIDE.md`](../../docs/EMBEDDED_GUIDE.md).
