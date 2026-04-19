//! Inspection-style steps that compute a value from a string/collection
//! without modifying it: `Length`, `Reverse`, `TypeOf`.

use super::super::super::compile_from_context::CompileFromContext;
use super::super::super::{CompileError, GremlinCompiler};

impl GremlinCompiler {
    pub(super) fn compile_str_length(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_length", "len")
    }

    pub(super) fn compile_str_reverse(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_reverse", "rev")
    }

    pub(super) fn compile_str_typeof(
        &mut self,
        ctx: &mut CompileFromContext,
    ) -> Result<(), CompileError> {
        self.emit_unary_str_projection(ctx, "gremlin_typeof", "type")
    }
}
