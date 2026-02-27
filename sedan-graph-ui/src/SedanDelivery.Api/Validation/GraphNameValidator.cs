using System.Text.RegularExpressions;

namespace SedanDelivery.Api.Validation;

/// <summary>
/// Validates graph names against the same constraint enforced by the Rust
/// gql-core engine: <c>[a-zA-Z0-9_.-]</c>, 1–64 characters.
/// </summary>
/// <remarks>
/// This is the single source of truth for graph name validation in the .NET
/// layer.  All controllers reference this class instead of duplicating the
/// pattern inline, ensuring consistency if the constraint is ever changed.
/// </remarks>
public static class GraphNameValidator
{
    /// <summary>Regex pattern matching valid graph names.</summary>
    public static readonly Regex Pattern =
        new(@"^[a-zA-Z0-9_.\-]{1,64}$", RegexOptions.Compiled, TimeSpan.FromMilliseconds(100));

    /// <summary>Returns <c>true</c> if <paramref name="name"/> is valid.</summary>
    public static bool IsValid(string? name) =>
        name is not null && Pattern.IsMatch(name);

    /// <summary>
    /// Returns a validation error message if the name is invalid, or
    /// <c>null</c> if it is valid.
    /// </summary>
    public static string? Validate(string? name) =>
        IsValid(name)
            ? null
            : $"Graph name must match {Pattern} (1–64 alphanumeric/underscore/dot/hyphen characters).";
}
