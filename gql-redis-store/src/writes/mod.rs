// writes/ -- All GraphStore write method implementations for RedisGraphStore.
//
// Split into focused sub-modules:
//   - helpers: Pure utility functions (ID extraction, property cleaning)
//   - nodes:   Node create/update/delete/batch operations
//   - edges:   Edge create/update/delete operations
//   - graphs:  Graph lifecycle (create_graph, delete_graph)

pub(crate) mod helpers;

mod edges;
mod graphs;
mod nodes;
