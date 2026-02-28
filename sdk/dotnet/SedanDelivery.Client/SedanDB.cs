using StackExchange.Redis;

namespace SedanDelivery.Client;

/// <summary>
/// Connection to a SedanDelivery server.
/// Wraps StackExchange.Redis ConnectionMultiplexer to provide a graph-native API.
/// </summary>
public class SedanDB : IDisposable
{
    private readonly ConnectionMultiplexer _redis;

    /// <summary>
    /// Connect to a SedanDelivery server using a Redis connection string.
    /// </summary>
    /// <param name="connectionString">
    /// Redis connection string (e.g., "localhost:6379" or "host:port,password=secret").
    /// </param>
    public SedanDB(string connectionString)
    {
        _redis = ConnectionMultiplexer.Connect(connectionString);
    }

    /// <summary>
    /// Connect using an existing ConnectionMultiplexer (useful for connection pooling).
    /// </summary>
    public SedanDB(ConnectionMultiplexer redis)
    {
        _redis = redis;
    }

    /// <summary>
    /// Select a named graph for querying.
    /// This does NOT create the graph; use graph.Create() for that.
    /// </summary>
    public Graph SelectGraph(string name) => new Graph(_redis.GetDatabase(), name);

    /// <summary>
    /// List all graph names on the server.
    /// </summary>
    public string[] ListGraphs()
    {
        var db = _redis.GetDatabase();
        var result = db.Execute("GQL.LIST-GRAPHS");
        return ((RedisResult[])result!).Select(r => (string)r!).ToArray();
    }

    /// <summary>
    /// List all graph names on the server (async).
    /// </summary>
    public async Task<string[]> ListGraphsAsync()
    {
        var db = _redis.GetDatabase();
        var result = await db.ExecuteAsync("GQL.LIST-GRAPHS");
        return ((RedisResult[])result!).Select(r => (string)r!).ToArray();
    }

    /// <summary>
    /// Dispose the underlying Redis connection.
    /// </summary>
    public void Dispose()
    {
        _redis.Dispose();
        GC.SuppressFinalize(this);
    }
}
