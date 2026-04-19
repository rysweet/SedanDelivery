// Graph-management commands: CREATE-GRAPH, DELETE-GRAPH, LIST-GRAPHS.

use redis_module::{Context, NextArg, RedisError, RedisResult, RedisValue};

use cypher_core::storage::GraphStore;

use crate::changelog::{discard_changelog, emit_changelog};
use crate::state::{clear_module_context, set_module_context, with_store};

/// GQL.CREATE-GRAPH <graph>
pub(crate) fn cmd_gql_create_graph(
    ctx: &Context,
    args: Vec<redis_module::RedisString>,
) -> RedisResult {
    set_module_context(ctx);
    let result = std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        let graph = args.next_string()?;
        args.done()?;

        with_store(|store| {
            store
                .create_graph(&graph)
                .map(|_| RedisValue::SimpleStringStatic("OK"))
                .map_err(|e| RedisError::String(format!("{e}")))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.CREATE-GRAPH: internal panic")));

    if result.is_ok() {
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emit_changelog(ctx);
        })) {
            tracing::warn!("GQL.CREATE-GRAPH: panic in emit_changelog: {e:?}");
        }
    } else if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        discard_changelog();
    })) {
        tracing::warn!("GQL.CREATE-GRAPH: panic in discard_changelog: {e:?}");
    }

    clear_module_context();
    result
}

/// GQL.DELETE-GRAPH <graph>
pub(crate) fn cmd_gql_delete_graph(
    ctx: &Context,
    args: Vec<redis_module::RedisString>,
) -> RedisResult {
    set_module_context(ctx);
    let result = std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        let graph = args.next_string()?;
        args.done()?;

        with_store(|store| {
            store
                .delete_graph(&graph)
                .map(|_| RedisValue::SimpleStringStatic("OK"))
                .map_err(|e| RedisError::String(format!("{e}")))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.DELETE-GRAPH: internal panic")));

    if result.is_ok() {
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emit_changelog(ctx);
        })) {
            tracing::warn!("GQL.DELETE-GRAPH: panic in emit_changelog: {e:?}");
        }
    } else if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        discard_changelog();
    })) {
        tracing::warn!("GQL.DELETE-GRAPH: panic in discard_changelog: {e:?}");
    }

    clear_module_context();
    result
}

/// GQL.LIST-GRAPHS
pub(crate) fn cmd_gql_list_graphs(
    ctx: &Context,
    args: Vec<redis_module::RedisString>,
) -> RedisResult {
    set_module_context(ctx);
    let result = std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        args.done()?;

        with_store(|store| {
            let names = store
                .list_graphs()
                .map_err(|e| RedisError::String(format!("{e}")))?;
            let values: Vec<RedisValue> = names.into_iter().map(RedisValue::BulkString).collect();
            Ok(RedisValue::Array(values))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.LIST-GRAPHS: internal panic")));
    clear_module_context();
    result
}
