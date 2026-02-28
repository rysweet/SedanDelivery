using System.Collections;
using System.Text.Json;
using StackExchange.Redis;

namespace SedanDelivery.Client;

/// <summary>
/// Structured result from a GQL.QUERY or GQL.MUTATE command.
/// Implements IEnumerable&lt;Record&gt; for LINQ and foreach support.
/// </summary>
public class QueryResult : IEnumerable<Record>
{
    /// <summary>The data records returned by the query.</summary>
    public IReadOnlyList<Record> Records { get; }

    /// <summary>Mutation statistics (zero for read-only queries).</summary>
    public MutationStats Stats { get; }

    /// <summary>Access a record by index.</summary>
    public Record this[int index] => Records[index];

    /// <summary>Number of records.</summary>
    public int Count => Records.Count;

    private QueryResult(IReadOnlyList<Record> records, MutationStats stats)
    {
        Records = records;
        Stats = stats;
    }

    /// <summary>
    /// Parse a GQL.QUERY response: RESP Array of BulkStrings, each being a JSON object.
    /// </summary>
    public static QueryResult FromQueryResponse(RedisResult[] results)
    {
        var records = new List<Record>(results.Length);
        foreach (var r in results)
        {
            var json = (string)r!;
            records.Add(ParseJsonRecord(json));
        }
        return new QueryResult(records.AsReadOnly(), MutationStats.Empty);
    }

    /// <summary>
    /// Parse a GQL.MUTATE response: RESP Array of [Array of JSON row BulkStrings, BulkString stats].
    /// </summary>
    public static QueryResult FromMutateResponse(RedisResult[] results)
    {
        // results[0] = Array of JSON row BulkStrings
        // results[1] = BulkString stats
        var rowResults = (RedisResult[])results[0]!;
        var statsString = (string)results[1]!;

        var records = new List<Record>(rowResults.Length);
        foreach (var r in rowResults)
        {
            var json = (string)r!;
            records.Add(ParseJsonRecord(json));
        }

        var stats = MutationStats.Parse(statsString);
        return new QueryResult(records.AsReadOnly(), stats);
    }

    private static Record ParseJsonRecord(string json)
    {
        using var doc = JsonDocument.Parse(json);
        var data = new Dictionary<string, object?>();

        foreach (var prop in doc.RootElement.EnumerateObject())
        {
            data[prop.Name] = TypeDecoder.Decode(prop.Value);
        }

        return new Record(data);
    }

    public IEnumerator<Record> GetEnumerator() => Records.GetEnumerator();
    IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();
}

/// <summary>
/// A single row from a query result. Provides typed access to column values.
/// </summary>
public class Record
{
    private readonly Dictionary<string, object?> _data;

    internal Record(Dictionary<string, object?> data)
    {
        _data = data;
    }

    /// <summary>
    /// Get a value by column name, converted to the specified type.
    /// Uses Convert.ChangeType for numeric/string conversions.
    /// </summary>
    public T Get<T>(string key)
    {
        var value = _data[key];
        if (value is null)
        {
            // For nullable value types and reference types, return default
            if (default(T) is null)
                return default!;
            throw new InvalidCastException($"Cannot convert null to {typeof(T).Name}");
        }

        if (value is T typed)
            return typed;

        // Handle common numeric conversions (e.g., long -> int, long -> double)
        return (T)Convert.ChangeType(value, typeof(T));
    }

    /// <summary>Access a value by column name (returns raw object).</summary>
    public object? this[string key] => _data[key];

    /// <summary>Try to get a value by column name.</summary>
    public bool TryGetValue(string key, out object? value) => _data.TryGetValue(key, out value);

    /// <summary>All column name/value pairs.</summary>
    public IReadOnlyDictionary<string, object?> Data => _data;
}

/// <summary>
/// Parsed mutation statistics from the wire format:
/// "nodes_created=N nodes_deleted=N edges_created=N edges_deleted=N properties_set=N labels_added=N"
/// </summary>
public class MutationStats
{
    public int NodesCreated { get; init; }
    public int NodesDeleted { get; init; }
    public int EdgesCreated { get; init; }
    public int EdgesDeleted { get; init; }
    public int PropertiesSet { get; init; }
    public int LabelsAdded { get; init; }

    /// <summary>An empty stats instance (all zeros).</summary>
    public static MutationStats Empty { get; } = new();

    /// <summary>
    /// Parse the stats string from GQL.MUTATE response.
    /// Format: "nodes_created=N nodes_deleted=N edges_created=N edges_deleted=N properties_set=N labels_added=N"
    /// </summary>
    public static MutationStats Parse(string statsString)
    {
        if (string.IsNullOrWhiteSpace(statsString))
            return Empty;

        int nodesCreated = 0, nodesDeleted = 0, edgesCreated = 0,
            edgesDeleted = 0, propertiesSet = 0, labelsAdded = 0;

        // Parse "key=value" pairs separated by spaces
        var pairs = statsString.Split(' ', StringSplitOptions.RemoveEmptyEntries);
        foreach (var pair in pairs)
        {
            var eqIdx = pair.IndexOf('=');
            if (eqIdx < 0) continue;

            var key = pair.Substring(0, eqIdx);
            if (!int.TryParse(pair.Substring(eqIdx + 1), out var value))
                continue;

            switch (key)
            {
                case "nodes_created": nodesCreated = value; break;
                case "nodes_deleted": nodesDeleted = value; break;
                case "edges_created": edgesCreated = value; break;
                case "edges_deleted": edgesDeleted = value; break;
                case "properties_set": propertiesSet = value; break;
                case "labels_added": labelsAdded = value; break;
            }
        }

        return new MutationStats
        {
            NodesCreated = nodesCreated,
            NodesDeleted = nodesDeleted,
            EdgesCreated = edgesCreated,
            EdgesDeleted = edgesDeleted,
            PropertiesSet = propertiesSet,
            LabelsAdded = labelsAdded,
        };
    }
}
