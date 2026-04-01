// sd2-valkey-module -- Valkey module for high-performance SedanDelivery
// graph MERGE operations.
//
// Provides the SD2.MERGE_EDGES command that executes the entire MERGE edge
// check-then-create cycle server-side in a single round-trip.
//
// Load with: MODULE LOAD /path/to/libsd2_valkey_module.so

pub mod keys;
pub mod merge;

// The valkey_module! macro registers a global allocator and the module entry
// point.  Both are incompatible with the standard test harness (no Valkey
// process is running), so we gate the entire macro behind `not(test)`.
// Unit tests exercise only the pure-logic helpers in keys.rs and merge.rs.
#[cfg(not(test))]
mod module_init {
    use valkey_module::{valkey_module, Context, Status, ValkeyString};

    fn on_load(ctx: &Context, _args: &[ValkeyString]) -> Status {
        ctx.log_notice("sd2-valkey-module loaded: SD2.MERGE_EDGES available");
        Status::Ok
    }

    valkey_module! {
        name: "sd2",
        version: 1,
        allocator: (valkey_module::alloc::ValkeyAlloc, valkey_module::alloc::ValkeyAlloc),
        data_types: [],
        init: on_load,
        commands: [
            // SD2.MERGE_EDGES {graph} {key_prefix} {num_edges} [edge_spec ...]
            // Flags: "write deny-oom" -- the command writes data and should be
            // rejected under OOM conditions.
            ["SD2.MERGE_EDGES", crate::merge::merge_edges, "write deny-oom", 0, 0, 0],
        ],
    }
}
