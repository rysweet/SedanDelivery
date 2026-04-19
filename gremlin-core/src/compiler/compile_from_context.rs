//! Mutable per-call state for `GremlinCompiler::compile_from`.
//!
//! The original `compile_from` function maintained ~50 local mutable variables
//! shared across a giant `for step in steps` loop. To enable decomposing the
//! per-arm bodies into separate methods, all of that state is bundled here.

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::Operator;

use super::helpers::WhereByTarget;
use super::SelectByModulator;
use crate::bytecode::GremlinStep;
use crate::types::GremlinValue;

/// All mutable state owned by a single `compile_from` invocation.
pub(super) struct CompileFromContext {
    pub current: Option<Operator>,
    pub current_alias: Identifier,
    pub pending_properties: Vec<(String, GremlinValue)>,
    pub pending_order: bool,
    pub pending_order_local: bool,
    pub pending_group: bool,
    pub pending_group_by_count: u32,
    pub pending_select: bool,
    pub select_by_mods: Vec<SelectByModulator>,
    pub pending_dedup: bool,
    pub dedup_by_consumed: bool,
    pub pending_aggregate: bool,
    pub pending_group_is_count: bool,
    pub aggregate_by_consumed: bool,
    pub pre_group_alias: Identifier,
    pub path_aliases: Vec<Identifier>,
    pub pending_emit: bool,
    pub pending_emit_pred: Option<Vec<GremlinStep>>,
    pub pending_until: Option<Vec<GremlinStep>>,
    pub pending_times: Option<usize>,
    pub pending_sack_init: Option<Expr>,
    pub pending_sack_merge_op: Option<String>,
    pub with_bulk_false: bool,
    pub pending_sack_op: bool,
    pub pending_project: bool,
    pub project_keys: Vec<String>,
    pub project_by_count: usize,
    pub pre_project_alias: Identifier,
    pub pending_format: bool,
    pub format_pattern: String,
    pub format_pre_alias: Identifier,
    pub format_by_aliases: Vec<Identifier>,
    pub adde_traverser: Option<Identifier>,
    pub pending_sample: bool,
    pub sample_by_consumed: bool,
    pub pending_sample_count: i64,
    pub pending_sample_weight: Option<Expr>,
    pub pending_sack_by_consumed: bool,
    pub pending_valuemap: bool,
    pub valuemap_by_consumed: bool,
    pub pending_path: bool,
    pub path_by_keys: Vec<String>,
    pub pending_choose_output: Option<Identifier>,
    pub pending_where_by_targets: Vec<WhereByTarget>,
    pub pending_where_by_modulators: Vec<String>,
    pub pending_math: bool,
    pub pending_math_expr: String,
    pub pre_math_alias: Identifier,
}

impl CompileFromContext {
    pub(super) fn new(
        initial_op: Option<Operator>,
        initial_alias: Identifier,
        initial_path_aliases: Vec<Identifier>,
    ) -> Self {
        Self {
            current: initial_op,
            current_alias: initial_alias.clone(),
            pending_properties: Vec::new(),
            pending_order: false,
            pending_order_local: false,
            pending_group: false,
            pending_group_by_count: 0,
            pending_select: false,
            select_by_mods: Vec::new(),
            pending_dedup: false,
            dedup_by_consumed: false,
            pending_aggregate: false,
            pending_group_is_count: false,
            aggregate_by_consumed: false,
            pre_group_alias: initial_alias.clone(),
            path_aliases: initial_path_aliases,
            pending_emit: false,
            pending_emit_pred: None,
            pending_until: None,
            pending_times: None,
            pending_sack_init: None,
            pending_sack_merge_op: None,
            with_bulk_false: false,
            pending_sack_op: false,
            pending_project: false,
            project_keys: Vec::new(),
            project_by_count: 0,
            pre_project_alias: initial_alias.clone(),
            pending_format: false,
            format_pattern: String::new(),
            format_pre_alias: initial_alias.clone(),
            format_by_aliases: Vec::new(),
            adde_traverser: None,
            pending_sample: false,
            sample_by_consumed: false,
            pending_sample_count: 0,
            pending_sample_weight: None,
            pending_sack_by_consumed: false,
            pending_valuemap: false,
            valuemap_by_consumed: false,
            pending_path: false,
            path_by_keys: Vec::new(),
            pending_choose_output: None,
            pending_where_by_targets: Vec::new(),
            pending_where_by_modulators: Vec::new(),
            pending_math: false,
            pending_math_expr: String::new(),
            pre_math_alias: initial_alias,
        }
    }
}
