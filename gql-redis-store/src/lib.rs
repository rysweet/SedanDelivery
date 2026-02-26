// gql-redis-store: Valkey/Redis-backed implementation of the GraphStore trait.
// Uses Redis hashes for node/edge properties, sorted sets for adjacency lists,
// and sets for label/reltype indexes. All graph keys use {graphname} hash tags
// for Redis Cluster slot affinity.
