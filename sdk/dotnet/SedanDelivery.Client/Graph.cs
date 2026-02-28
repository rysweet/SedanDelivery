using System.Globalization;
using System.Reflection;
using System.Text;
using StackExchange.Redis;

namespace SedanDelivery.Client;

/// <summary>
/// Represents a named graph within SedanDelivery.
/// Provides sync and async methods for querying and mutating graph data.
/// </summary>
public class Graph
{
    private readonly IDatabase _db;
    private readonly string _name;

    /// <summary>The name of this graph.</summary>
    public string Name => _name;

    internal Graph(IDatabase db, string name)
    {
        _db = db;
        _name = name;
    }

    /// <summary>
    /// Execute a read-only Cypher query.
    /// </summary>
    /// <param name="cypher">The Cypher query string. Use $paramName for parameter placeholders.</param>
    /// <param name="parameters">Optional anonymous object or Dictionary with parameter values.</param>
    public QueryResult Query(string cypher, object? parameters = null)
    {
        var query = SubstituteParams(cypher, parameters);
        var result = _db.Execute("GQL.QUERY", _name, query);
        return QueryResult.FromQueryResponse((RedisResult[])result!);
    }

    /// <summary>
    /// Execute a write (mutating) Cypher query.
    /// </summary>
    /// <param name="cypher">The Cypher query string. Use $paramName for parameter placeholders.</param>
    /// <param name="parameters">Optional anonymous object or Dictionary with parameter values.</param>
    public QueryResult Mutate(string cypher, object? parameters = null)
    {
        var query = SubstituteParams(cypher, parameters);
        var result = _db.Execute("GQL.MUTATE", _name, query);
        return QueryResult.FromMutateResponse((RedisResult[])result!);
    }

    /// <summary>
    /// Execute a read-only Cypher query asynchronously.
    /// </summary>
    public async Task<QueryResult> QueryAsync(string cypher, object? parameters = null)
    {
        var query = SubstituteParams(cypher, parameters);
        var result = await _db.ExecuteAsync("GQL.QUERY", _name, query);
        return QueryResult.FromQueryResponse((RedisResult[])result!);
    }

    /// <summary>
    /// Execute a write (mutating) Cypher query asynchronously.
    /// </summary>
    public async Task<QueryResult> MutateAsync(string cypher, object? parameters = null)
    {
        var query = SubstituteParams(cypher, parameters);
        var result = await _db.ExecuteAsync("GQL.MUTATE", _name, query);
        return QueryResult.FromMutateResponse((RedisResult[])result!);
    }

    /// <summary>Create this graph on the server.</summary>
    public void Create() => _db.Execute("GQL.CREATE-GRAPH", _name);

    /// <summary>Delete this graph from the server.</summary>
    public void Delete() => _db.Execute("GQL.DELETE-GRAPH", _name);

    /// <summary>Return the query execution plan as a string.</summary>
    public string Explain(string cypher) => (string)_db.Execute("GQL.EXPLAIN", _name, cypher)!;

    /// <summary>Create this graph on the server (async).</summary>
    public async Task CreateAsync() => await _db.ExecuteAsync("GQL.CREATE-GRAPH", _name);

    /// <summary>Delete this graph from the server (async).</summary>
    public async Task DeleteAsync() => await _db.ExecuteAsync("GQL.DELETE-GRAPH", _name);

    /// <summary>
    /// Client-side parameter substitution.
    ///
    /// Replaces $paramName tokens in the Cypher string with literal values from the
    /// parameters object. Supports:
    ///   - null -> "null"
    ///   - bool -> "true" / "false"
    ///   - int/long/double -> number literal
    ///   - string -> single-quoted, with ' escaped as \'
    ///   - Anonymous objects -> extract properties via reflection
    ///   - Dictionary&lt;string, object?&gt; -> key/value pairs
    /// </summary>
    internal static string SubstituteParams(string cypher, object? parameters)
    {
        if (parameters is null)
            return cypher;

        var paramDict = ExtractParameters(parameters);
        if (paramDict.Count == 0)
            return cypher;

        // Sort parameter names by length descending to prevent $name matching before $nameLong
        var sortedKeys = paramDict.Keys.OrderByDescending(k => k.Length).ToList();

        var result = new StringBuilder(cypher);
        foreach (var key in sortedKeys)
        {
            var placeholder = "$" + key;
            var literal = FormatValue(paramDict[key]);
            result.Replace(placeholder, literal);
        }

        return result.ToString();
    }

    private static Dictionary<string, object?> ExtractParameters(object parameters)
    {
        if (parameters is Dictionary<string, object?> dict)
            return dict;

        if (parameters is IDictionary<string, object?> idict)
            return new Dictionary<string, object?>(idict);

        // Anonymous object or regular object: extract public instance properties
        var result = new Dictionary<string, object?>();
        var props = parameters.GetType().GetProperties(BindingFlags.Public | BindingFlags.Instance);
        foreach (var prop in props)
        {
            if (prop.CanRead)
            {
                result[prop.Name] = prop.GetValue(parameters);
            }
        }
        return result;
    }

    private static string FormatValue(object? value)
    {
        if (value is null)
            return "null";

        if (value is bool b)
            return b ? "true" : "false";

        if (value is int i)
            return i.ToString(CultureInfo.InvariantCulture);

        if (value is long l)
            return l.ToString(CultureInfo.InvariantCulture);

        if (value is double d)
            return d.ToString(CultureInfo.InvariantCulture);

        if (value is float f)
            return f.ToString(CultureInfo.InvariantCulture);

        if (value is decimal dec)
            return dec.ToString(CultureInfo.InvariantCulture);

        if (value is string s)
            return "'" + s.Replace("\\", "\\\\").Replace("'", "\\'") + "'";

        // Fallback: use ToString with single quotes for safety
        return "'" + value.ToString()!.Replace("\\", "\\\\").Replace("'", "\\'") + "'";
    }
}
