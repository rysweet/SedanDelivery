// sd2-redis-module: Valkey/Redis module entry point.
//
// Exposes the following commands:
//   GQL.QUERY <graph> <query>       -- execute read query, return JSON rows
//   GQL.MUTATE <graph> <query>      -- execute write query, return OK + stats
//   GQL.CREATE-GRAPH <graph>        -- create a named graph
//   GQL.DELETE-GRAPH <graph>        -- delete a named graph
//   GQL.LIST-GRAPHS                 -- list all graph names
//   GQL.EXPLAIN <graph> <query>     -- return the physical plan as a string
//
// Safety: every command handler uses std::panic::catch_unwind to prevent a
// panic from killing the Redis/Valkey process (and all connected clients).
//
// Architecture: graph data is stored in Redis via ModuleRedisStore which uses
// ctx.call() (the Valkey Module internal dispatch) to execute Redis commands.
// This avoids the TCP loopback deadlock that prevents a regular RedisGraphStore
// from working inside a module, while using the EXACT same key schema so that
// the Bolt server, REST API, and GQL module all read/write the same data.
//
// ModuleRedisStore is wrapped in a ChangeLogStore that records every write as
// a ChangeEvent flushed to the __sd2:changelog Valkey stream after each
// successful write command (when GQL_FABRIC_CHANGELOG=1 is set).
//
// Changelog: set GQL_FABRIC_CHANGELOG=1 (or GQL_FABRIC_ENDPOINT=<url>) at
// module load time to enable async replication to Microsoft Fabric via the
// sedandelivery-fabric-sync sidecar service.
//
// File layout:
//   - state.rs           : global STORE / CHANGELOG_ENABLED + lifecycle helpers
//   - changelog.rs       : emit_changelog / discard_changelog
//   - commands/          : one file per GQL.* command handler
//   - module_store/      : ModuleRedisStore (Valkey Module API GraphStore)

mod changelog;
mod commands;
mod module_store;
mod state;

use redis_module::redis_module;

use commands::{
    cmd_gql_create_graph, cmd_gql_delete_graph, cmd_gql_explain, cmd_gql_list_graphs,
    cmd_gql_mutate, cmd_gql_query,
};
use state::module_init;

// -- Module registration --

redis_module! {
    name: "gql",
    version: 1,
    allocator: (redis_module::alloc::RedisAlloc, redis_module::alloc::RedisAlloc),
    data_types: [],
    init: module_init,
    commands: [
        ["GQL.QUERY",        cmd_gql_query,        "readonly",  1, 1, 1],
        ["GQL.MUTATE",       cmd_gql_mutate,       "write",     1, 1, 1],
        ["GQL.CREATE-GRAPH", cmd_gql_create_graph, "write",     1, 1, 1],
        ["GQL.DELETE-GRAPH", cmd_gql_delete_graph, "write",     1, 1, 1],
        ["GQL.LIST-GRAPHS",  cmd_gql_list_graphs,  "readonly",  0, 0, 0],
        ["GQL.EXPLAIN",      cmd_gql_explain,      "readonly",  1, 1, 1],
    ],
}
