// gql-module: Valkey/Redis module entry point.
// Exposes GQL.QUERY, GQL.MUTATE, and related commands as Redis module commands.
// Delegates to gql-core for parsing, planning, and execution,
// and to gql-redis-store for the Redis-backed GraphStore implementation.
