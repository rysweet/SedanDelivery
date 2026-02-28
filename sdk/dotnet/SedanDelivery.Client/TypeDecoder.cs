using System.Globalization;
using System.Text.Json;

namespace SedanDelivery.Client;

/// <summary>
/// A geographic point with coordinate reference system tag.
/// </summary>
public record Point(double X, double Y, string Crs = "Wgs84");

/// <summary>
/// A graph path: alternating node IDs and edge IDs, starting and ending with a node.
/// </summary>
public record SedanPath(long[] Nodes, long[] Edges);

/// <summary>
/// Decodes tagged string values from the SedanDelivery wire format into .NET types.
///
/// Tagged value encoding:
///   __date:YYYY-MM-DD           -> DateOnly
///   __localtime:HH:mm:ss[.fff]  -> TimeOnly
///   __localdatetime:...          -> DateTime (DateTimeKind.Unspecified)
///   __datetime:...               -> DateTimeOffset
///   __duration:P...              -> TimeSpan (limited: days + time only)
///   __point:x,y,crs             -> Point record
///   __node:id                   -> long
///   __edge:id                   -> long
///   __str:...                   -> string (unescaped)
///   Path objects ({__path_nodes, __path_edges}) -> SedanPath record
/// </summary>
public static class TypeDecoder
{
    /// <summary>
    /// Decode a single JSON element into a .NET object, applying tagged-string decoding.
    /// </summary>
    public static object? Decode(JsonElement element)
    {
        switch (element.ValueKind)
        {
            case JsonValueKind.Null:
            case JsonValueKind.Undefined:
                return null;

            case JsonValueKind.True:
                return true;

            case JsonValueKind.False:
                return false;

            case JsonValueKind.Number:
                // Prefer long for integers, double for fractional
                if (element.TryGetInt64(out long longVal))
                    return longVal;
                return element.GetDouble();

            case JsonValueKind.String:
                return DecodeString(element.GetString()!);

            case JsonValueKind.Array:
                return DecodeArray(element);

            case JsonValueKind.Object:
                return DecodeObject(element);

            default:
                return element.GetRawText();
        }
    }

    /// <summary>
    /// Decode a tagged string value, or return the plain string if no tag matches.
    /// </summary>
    private static object DecodeString(string value)
    {
        // __str: escape -- must be checked first to prevent user strings
        // starting with __ from being misinterpreted as tags.
        if (value.StartsWith("__str:", StringComparison.Ordinal))
            return value.Substring(6);

        if (value.StartsWith("__date:", StringComparison.Ordinal))
        {
            var rest = value.Substring(7);
            if (DateOnly.TryParseExact(rest, "yyyy-MM-dd", CultureInfo.InvariantCulture, DateTimeStyles.None, out var date))
                return date;
            // Fallback: try standard parse
            if (DateOnly.TryParse(rest, CultureInfo.InvariantCulture, DateTimeStyles.None, out date))
                return date;
            return value; // unparseable, return raw
        }

        if (value.StartsWith("__localtime:", StringComparison.Ordinal))
        {
            var rest = value.Substring(12);
            if (TimeOnly.TryParse(rest, CultureInfo.InvariantCulture, DateTimeStyles.None, out var time))
                return time;
            return value;
        }

        if (value.StartsWith("__localdatetime:", StringComparison.Ordinal))
        {
            var rest = value.Substring(16);
            if (DateTime.TryParse(rest, CultureInfo.InvariantCulture, DateTimeStyles.None, out var dt))
                return DateTime.SpecifyKind(dt, DateTimeKind.Unspecified);
            return value;
        }

        if (value.StartsWith("__datetime:", StringComparison.Ordinal))
        {
            var rest = value.Substring(11);
            // Strip IANA timezone bracket if present, e.g. "2024-01-15T10:30:00+01:00[Europe/Berlin]"
            var bracketIdx = rest.IndexOf('[');
            var parseStr = bracketIdx >= 0 ? rest.Substring(0, bracketIdx) : rest;
            if (DateTimeOffset.TryParse(parseStr, CultureInfo.InvariantCulture, DateTimeStyles.None, out var dto))
                return dto;
            return value;
        }

        if (value.StartsWith("__duration:", StringComparison.Ordinal))
        {
            var rest = value.Substring(11);
            return ParseIsoDuration(rest);
        }

        if (value.StartsWith("__point:", StringComparison.Ordinal))
        {
            var rest = value.Substring(8);
            var parts = rest.Split(',', 3);
            if (parts.Length == 3
                && double.TryParse(parts[0], NumberStyles.Float, CultureInfo.InvariantCulture, out var x)
                && double.TryParse(parts[1], NumberStyles.Float, CultureInfo.InvariantCulture, out var y))
            {
                return new Point(x, y, parts[2]);
            }
            return value;
        }

        if (value.StartsWith("__node:", StringComparison.Ordinal))
        {
            var rest = value.Substring(7);
            if (long.TryParse(rest, out var id))
                return id;
            return value;
        }

        if (value.StartsWith("__edge:", StringComparison.Ordinal))
        {
            var rest = value.Substring(7);
            if (long.TryParse(rest, out var id))
                return id;
            return value;
        }

        // Plain string
        return value;
    }

    /// <summary>
    /// Parse a limited ISO 8601 duration string (P[nY][nM][nW][nD][T[nH][nM][n[.n]S]]) into TimeSpan.
    /// Note: TimeSpan cannot represent months/years, so those are approximated (30 days/month, 365 days/year).
    /// </summary>
    private static TimeSpan ParseIsoDuration(string iso)
    {
        var totalDays = 0.0;
        var totalTicks = 0L;
        var inTimePart = false;
        var numBuf = "";

        for (int i = 0; i < iso.Length; i++)
        {
            var c = iso[i];
            if (c == 'P') continue;
            if (c == 'T') { inTimePart = true; continue; }

            if (c == '-' || c == '.' || char.IsDigit(c))
            {
                numBuf += c;
                continue;
            }

            if (!double.TryParse(numBuf, NumberStyles.Float, CultureInfo.InvariantCulture, out var num))
            {
                numBuf = "";
                continue;
            }
            numBuf = "";

            if (inTimePart)
            {
                switch (c)
                {
                    case 'H': totalTicks += (long)(num * TimeSpan.TicksPerHour); break;
                    case 'M': totalTicks += (long)(num * TimeSpan.TicksPerMinute); break;
                    case 'S': totalTicks += (long)(num * TimeSpan.TicksPerSecond); break;
                }
            }
            else
            {
                switch (c)
                {
                    case 'Y': totalDays += num * 365; break;
                    case 'M': totalDays += num * 30; break;
                    case 'W': totalDays += num * 7; break;
                    case 'D': totalDays += num; break;
                }
            }
        }

        return TimeSpan.FromDays(totalDays) + TimeSpan.FromTicks(totalTicks);
    }

    private static object?[] DecodeArray(JsonElement element)
    {
        var result = new object?[element.GetArrayLength()];
        int i = 0;
        foreach (var item in element.EnumerateArray())
        {
            result[i++] = Decode(item);
        }
        return result;
    }

    private static object DecodeObject(JsonElement element)
    {
        // Check for encoded path: { "__path_nodes": [...], "__path_edges": [...] }
        if (element.TryGetProperty("__path_nodes", out var nodesEl)
            && element.TryGetProperty("__path_edges", out var edgesEl))
        {
            var nodes = nodesEl.EnumerateArray().Select(e => e.GetInt64()).ToArray();
            var edges = edgesEl.EnumerateArray().Select(e => e.GetInt64()).ToArray();
            return new SedanPath(nodes, edges);
        }

        // Regular object: decode into Dictionary
        var dict = new Dictionary<string, object?>();
        foreach (var prop in element.EnumerateObject())
        {
            dict[prop.Name] = Decode(prop.Value);
        }
        return dict;
    }
}
