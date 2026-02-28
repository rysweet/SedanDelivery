# SedanDelivery Client SDKs

Client libraries for the SedanDelivery graph database engine.
All three SDKs communicate with a running SedanDelivery server over the Redis
protocol, wrapping the `GQL.*` custom commands behind a typed, ergonomic API.

| SDK | Language | Package | Minimum runtime |
|-----|----------|---------|-----------------|
| [Python](#python-sdk) | Python 3.9+ | `sedandelivery` | CPython / PyPy |
| [.NET](#net-sdk) | C# / F# | `SedanDelivery.Client` | .NET 8.0+ |
| [Rust](#rust-sdk) | Rust | `sedan-client` | Rust 1.70+ (2021 edition) |

---

## Python SDK

### Installation

```bash
pip install sedandelivery          # from PyPI (when published)
pip install -e sdk/python          # editable install from source
```

Requires `redis >= 5.0` (pulled automatically).

### Quick start

```python
from sedandelivery import SedanDB

db = SedanDB(host="localhost", port=6379, password="secret")
g = db.select_graph("social")
g.create()
g.mutate("CREATE (:Person {name: $name, age: $age})", name="Alice", age=30)

for r in g.query("MATCH (n:Person) RETURN n.name, n.age"):
    print(r["n.name"], r["n.age"])

g.delete()
db.close()
```

### API reference

#### `SedanDB`

| Method | Signature | Returns | Description |
|--------|-----------|---------|-------------|
| `__init__` | `(host="localhost", port=6379, password=None, **redis_kwargs)` | `SedanDB` | Connect to a SedanDelivery server. |
| `from_url` | `(url: str) -> SedanDB` | `SedanDB` | Class method. Connect via Redis URL (e.g. `redis://host:6379/0`). |
| `select_graph` | `(name: str) -> Graph` | `Graph` | Return a handle for the named graph. |
| `list_graphs` | `() -> list[str]` | `list[str]` | List all graph names on the server. |
| `close` | `() -> None` | `None` | Close the Redis connection. |

`SedanDB` implements the context-manager protocol (`with SedanDB() as db: ...`).

#### `Graph`

| Method | Signature | Returns | Description |
|--------|-----------|---------|-------------|
| `create` | `() -> None` | `None` | Create this graph on the server (`GQL.CREATE-GRAPH`). |
| `delete` | `() -> None` | `None` | Delete this graph from the server (`GQL.DELETE-GRAPH`). |
| `query` | `(cypher: str, **params) -> QueryResult` | `QueryResult` | Execute a read-only Cypher query (`GQL.QUERY`). |
| `mutate` | `(cypher: str, **params) -> QueryResult` | `QueryResult` | Execute a mutating Cypher query (`GQL.MUTATE`). |
| `explain` | `(cypher: str) -> str` | `str` | Return the execution plan for a query (`GQL.EXPLAIN`). |

**Attribute:** `name: str` -- the graph name this handle is bound to.

#### `QueryResult`

| Member | Type / Signature | Description |
|--------|------------------|-------------|
| `stats` | `MutationStats` | Mutation statistics (zero for read-only queries). |
| `len()` | `int` | Number of records. |
| `[index]` | `Record` | Access a record by position. |
| iteration | `Iterator[Record]` | Iterate over all records with `for r in result`. |

#### `Record`

| Method | Signature | Returns | Description |
|--------|-----------|---------|-------------|
| `__getitem__` | `(key: str) -> Any` | `Any` | Get column value by name. Raises `KeyError` if absent. |
| `get` | `(key: str, default=None) -> Any` | `Any` | Get column value, or *default* if absent. |
| `data` | `() -> dict[str, Any]` | `dict` | Return the full row as a dict. |
| `keys` | `() -> list[str]` | `list[str]` | Return column names. |

#### `MutationStats`

| Attribute | Type | Description |
|-----------|------|-------------|
| `nodes_created` | `int` | Number of nodes created. |
| `nodes_deleted` | `int` | Number of nodes deleted. |
| `edges_created` | `int` | Number of edges (relationships) created. |
| `edges_deleted` | `int` | Number of edges deleted. |
| `properties_set` | `int` | Number of properties set. |
| `labels_added` | `int` | Number of labels added. |

#### `Point` and `Path`

| Type | Fields | Description |
|------|--------|-------------|
| `Point` | `x: float, y: float, crs: str` | Geographic or Cartesian point (namedtuple). |
| `Path` | `nodes: list[int], edges: list[int]` | Graph path of node/edge IDs (namedtuple). |

---

## .NET SDK

### Installation

```bash
dotnet add package SedanDelivery.Client    # from NuGet (when published)
```

Or reference the project directly:

```xml
<ProjectReference Include="../sdk/dotnet/SedanDelivery.Client/SedanDelivery.Client.csproj" />
```

Requires `StackExchange.Redis >= 2.8` and `System.Text.Json >= 9.0` (pulled
automatically). Targets .NET 8.0.

### Quick start

```csharp
using SedanDelivery.Client;

using var db = new SedanDB("localhost:6379,password=secret");
var g = db.SelectGraph("social");
g.Create();
g.Mutate("CREATE (:Person {name: $name, age: $age})", new { name = "Alice", age = 30 });

foreach (var r in g.Query("MATCH (n:Person) RETURN n.name, n.age"))
    Console.WriteLine($"{r.Get<string>("n.name")} is {r.Get<int>("n.age")}");

g.Delete();
```

### API reference

#### `SedanDB` (implements `IDisposable`)

| Method | Signature | Returns | Description |
|--------|-----------|---------|-------------|
| constructor | `SedanDB(string connectionString)` | `SedanDB` | Connect via Redis connection string. |
| constructor | `SedanDB(ConnectionMultiplexer redis)` | `SedanDB` | Wrap an existing multiplexer (connection pooling). |
| `SelectGraph` | `(string name) -> Graph` | `Graph` | Return a handle for the named graph. |
| `ListGraphs` | `() -> string[]` | `string[]` | List all graph names on the server. |
| `ListGraphsAsync` | `() -> Task<string[]>` | `Task<string[]>` | Async variant of `ListGraphs`. |
| `Dispose` | `()` | `void` | Close the Redis connection. |

#### `Graph`

| Method | Signature | Returns | Description |
|--------|-----------|---------|-------------|
| `Create` | `()` | `void` | Create this graph on the server. |
| `CreateAsync` | `()` | `Task` | Async variant. |
| `Delete` | `()` | `void` | Delete this graph from the server. |
| `DeleteAsync` | `()` | `Task` | Async variant. |
| `Query` | `(string cypher, object? parameters = null)` | `QueryResult` | Execute a read-only Cypher query. |
| `QueryAsync` | `(string cypher, object? parameters = null)` | `Task<QueryResult>` | Async variant. |
| `Mutate` | `(string cypher, object? parameters = null)` | `QueryResult` | Execute a mutating Cypher query. |
| `MutateAsync` | `(string cypher, object? parameters = null)` | `Task<QueryResult>` | Async variant. |
| `Explain` | `(string cypher)` | `string` | Return the execution plan. |

**Property:** `Name: string` -- the graph name.

#### `QueryResult` (implements `IEnumerable<Record>`)

| Member | Type / Signature | Description |
|--------|------------------|-------------|
| `Records` | `IReadOnlyList<Record>` | All data records. |
| `Stats` | `MutationStats` | Mutation statistics. |
| `Count` | `int` | Number of records. |
| `[index]` | `Record` | Access a record by position. |
| `GetEnumerator()` | `IEnumerator<Record>` | For `foreach` / LINQ. |

#### `Record`

| Method | Signature | Returns | Description |
|--------|-----------|---------|-------------|
| `Get<T>` | `(string key) -> T` | `T` | Get a typed value by column name. Uses `Convert.ChangeType` for numeric conversions. |
| `[key]` | indexer | `object?` | Get raw value by column name. |
| `TryGetValue` | `(string key, out object? value) -> bool` | `bool` | Try to get a value. |
| `Data` | property | `IReadOnlyDictionary<string, object?>` | All column name/value pairs. |

#### `MutationStats`

| Property | Type | Description |
|----------|------|-------------|
| `NodesCreated` | `int` | Number of nodes created. |
| `NodesDeleted` | `int` | Number of nodes deleted. |
| `EdgesCreated` | `int` | Number of edges created. |
| `EdgesDeleted` | `int` | Number of edges deleted. |
| `PropertiesSet` | `int` | Number of properties set. |
| `LabelsAdded` | `int` | Number of labels added. |
| `Empty` | `MutationStats` (static) | Pre-allocated zero-valued instance. |

#### `Point` and `SedanPath`

| Type | Fields | Description |
|------|--------|-------------|
| `Point` | `X: double, Y: double, Crs: string` | Geographic or Cartesian point (record type). |
| `SedanPath` | `Nodes: long[], Edges: long[]` | Graph path of node/edge IDs (record type). |

---

## Rust SDK

### Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
sedan-client = { path = "sdk/rust/sedan-client" }
```

The crate depends on the `redis` crate for the connection layer.

### Quick start

```rust
use sedan_client::SedanDB;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = SedanDB::new("redis://localhost:6379")?;
    let mut g = db.select_graph("social")?;
    g.create()?;
    g.mutate("CREATE (:Person {name: 'Alice', age: 30})")?;

    for record in g.query("MATCH (n:Person) RETURN n.name, n.age")? {
        println!("{} is {}", record.get_str("n.name").unwrap(), record.get_i64("n.age").unwrap());
    }

    g.delete()?;
    Ok(())
}
```

### API reference

#### `SedanDB`

| Method | Signature | Returns | Description |
|--------|-----------|---------|-------------|
| `new` | `(url: &str) -> Result<SedanDB>` | `Result<SedanDB>` | Connect to a SedanDelivery server via Redis URL. |
| `select_graph` | `(&self, name: &str) -> Result<Graph>` | `Result<Graph>` | Return a handle for the named graph. |
| `list_graphs` | `(&self) -> Result<Vec<String>>` | `Result<Vec<String>>` | List all graph names on the server. |

#### `Graph`

| Method | Signature | Returns | Description |
|--------|-----------|---------|-------------|
| `create` | `(&mut self) -> Result<()>` | `Result<()>` | Create this graph on the server. |
| `delete` | `(&mut self) -> Result<()>` | `Result<()>` | Delete this graph from the server. |
| `query` | `(&mut self, cypher: &str) -> Result<QueryResult>` | `Result<QueryResult>` | Execute a read-only Cypher query. |
| `mutate` | `(&mut self, cypher: &str) -> Result<QueryResult>` | `Result<QueryResult>` | Execute a mutating Cypher query. |
| `explain` | `(&mut self, cypher: &str) -> Result<String>` | `Result<String>` | Return the execution plan. |
| `name` | `(&self) -> &str` | `&str` | The graph name. |

#### `QueryResult`

| Method / Field | Type / Signature | Description |
|----------------|------------------|-------------|
| `records` | `Vec<Record>` | All data records. |
| `stats` | `MutationStats` | Mutation statistics. |
| `len` | `(&self) -> usize` | Number of records. |
| `is_empty` | `(&self) -> bool` | True if no records. |
| `iter` | `(&self) -> impl Iterator<Item = &Record>` | Iterate over records. |
| `into_iter` | `(self) -> impl Iterator<Item = Record>` | Consuming iterator. |

#### `Record`

| Method | Signature | Returns | Description |
|--------|-----------|---------|-------------|
| `get` | `(&self, key: &str) -> Option<&Value>` | `Option<&Value>` | Get raw value by column name. |
| `get_str` | `(&self, key: &str) -> Option<&str>` | `Option<&str>` | Get value as string. |
| `get_i64` | `(&self, key: &str) -> Option<i64>` | `Option<i64>` | Get value as 64-bit integer. |
| `get_f64` | `(&self, key: &str) -> Option<f64>` | `Option<f64>` | Get value as 64-bit float. |
| `get_bool` | `(&self, key: &str) -> Option<bool>` | `Option<bool>` | Get value as boolean. |
| `keys` | `(&self) -> impl Iterator<Item = &str>` | iterator | Column names. |
| `data` | `(&self) -> &HashMap<String, Value>` | `&HashMap` | All column name/value pairs. |

#### `MutationStats`

| Field | Type | Description |
|-------|------|-------------|
| `nodes_created` | `i64` | Number of nodes created. |
| `nodes_deleted` | `i64` | Number of nodes deleted. |
| `edges_created` | `i64` | Number of edges created. |
| `edges_deleted` | `i64` | Number of edges deleted. |
| `properties_set` | `i64` | Number of properties set. |
| `labels_added` | `i64` | Number of labels added. |

#### `Value` (enum)

| Variant | Wrapped type | Description |
|---------|-------------|-------------|
| `Null` | -- | Cypher `null`. |
| `Bool(bool)` | `bool` | Boolean. |
| `Int(i64)` | `i64` | Integer. |
| `Float(f64)` | `f64` | Floating-point number. |
| `Str(String)` | `String` | String. |
| `List(Vec<Value>)` | `Vec<Value>` | Ordered list. |
| `Map(HashMap<String, Value>)` | `HashMap<String, Value>` | Key-value map. |
| `Point { x: f64, y: f64, crs: String }` | struct variant | Geographic / Cartesian point. |
| `Path { nodes: Vec<i64>, edges: Vec<i64> }` | struct variant | Graph path. |

---

## Parameter substitution

All three SDKs perform **client-side** parameter substitution. The `$paramName`
placeholders in Cypher strings are replaced with escaped literal values before
sending the command to the server.

### Rules

1. Parameters are referenced with `$name` in the Cypher string.
2. Only parameters provided by the caller are replaced; unknown `$` references
   are left as-is (the server reports the error).
3. String values are single-quoted and escaped (`\` -> `\\`, `'` -> `\'`).
4. `null`, booleans, and numbers are rendered as Cypher literals.
5. In Python, pass parameters as keyword arguments: `g.query("...", name="Alice")`.
6. In .NET, pass an anonymous object or `Dictionary<string, object?>`: `g.Query("...", new { name = "Alice" })`.
7. In Rust, parameter substitution is not yet built into the SDK; inline values
   directly in the Cypher string or use `format!` before calling `query`/`mutate`.

### Example

```
Cypher:   MATCH (n) WHERE n.name = $name AND n.age > $min RETURN n
Params:   name="Alice", min=25
Sent:     MATCH (n) WHERE n.name = 'Alice' AND n.age > 25 RETURN n
```

---

## Type mapping

How Cypher types map to each SDK's native types.

| Cypher type | Python | C# (.NET) | Rust |
|-------------|--------|-----------|------|
| `null` | `None` | `null` | `Value::Null` |
| `Boolean` | `bool` | `bool` | `Value::Bool(bool)` |
| `Integer` | `int` | `long` | `Value::Int(i64)` |
| `Float` | `float` | `double` | `Value::Float(f64)` |
| `String` | `str` | `string` | `Value::Str(String)` |
| `List` | `list` | `object?[]` | `Value::List(Vec<Value>)` |
| `Map` | `dict` | `Dictionary<string, object?>` | `Value::Map(HashMap<String, Value>)` |
| `Date` | `datetime.date` | `DateOnly` | `Value::Str(String)` |
| `LocalTime` | `datetime.time` | `TimeOnly` | `Value::Str(String)` |
| `LocalDateTime` | `datetime.datetime` (no tz) | `DateTime` (Unspecified) | `Value::Str(String)` |
| `DateTime` | `datetime.datetime` (with tz) | `DateTimeOffset` | `Value::Str(String)` |
| `Duration` | `str` (ISO 8601 raw) | `TimeSpan` (approx.) | `Value::Str(String)` |
| `Point` | `Point(x, y, crs)` | `Point(X, Y, Crs)` | `Value::Point { x, y, crs }` |
| `Path` | `Path(nodes, edges)` | `SedanPath(Nodes, Edges)` | `Value::Path { nodes, edges }` |
| `Node ID` | `int` | `long` | `Value::Int(i64)` |
| `Edge ID` | `int` | `long` | `Value::Int(i64)` |

---

## Live testing

### Prerequisites

A running SedanDelivery server. The quickest way:

```bash
cargo run -p gql-cli                      # in-process demo (no server needed)
# or
docker compose up -d                      # if a compose file is available
# or deploy to Azure (see main README)
```

### Python

```bash
cd sdk/python
pip install -e ".[dev]"
pytest tests/ -v                          # unit tests (mocked Redis)

# Integration test against a live server:
SEDAN_HOST=localhost SEDAN_PORT=6379 SEDAN_PASSWORD=secret \
  pytest tests/ -v -k integration
```

### .NET

```bash
cd sdk/dotnet/SedanDelivery.Client
dotnet build
dotnet test                               # if a test project exists

# Quick smoke test against a live server:
# (create a console app that references the project and run it)
```

### Rust

```bash
cd sdk/rust/sedan-client
cargo build
cargo test                                # unit tests

# Integration test against a live server:
SEDAN_URL=redis://localhost:6379 cargo test -- --ignored
```

---

## Project layout

```
sdk/
  python/
    pyproject.toml             # package metadata (hatchling)
    sedandelivery/
      __init__.py              # public exports
      client.py                # SedanDB class
      graph.py                 # Graph class, parameter substitution
      result.py                # QueryResult, Record, MutationStats
      types.py                 # Point, Path, tagged-value decoding
    tests/
      test_sdk.py
  dotnet/
    SedanDelivery.Client/
      SedanDelivery.Client.csproj
      SedanDB.cs               # SedanDB class
      Graph.cs                 # Graph class, parameter substitution
      QueryResult.cs           # QueryResult, Record, MutationStats
      TypeDecoder.cs           # Point, SedanPath, tagged-value decoding
  rust/
    sedan-client/              # (planned)
      Cargo.toml
      src/lib.rs
```
