//! `strings` category step compilation arms extracted from `dispatch.rs`.
//!
//! Pure mechanical extraction — no behavior changes. The original monolithic
//! match in `strings.rs` has been split into topic-focused submodules:
//!
//! * [`case_trim`] — `ToLower`, `ToUpper`, `Trim`, `LTrim`, `RTrim`
//! * [`inspect`]   — `Length`, `Reverse`, `TypeOf`
//! * [`mutate`]    — `Replace`, `Split`, `Substring`, `SubstringLocal`
//! * [`concat`]    — `Concat`, `Conjoin`, `Format`, `StringOpLocal`
//! * [`conversion`] — `AsString`, `AsStringLocal`, `AsNumber`,
//!   `AsNumberTyped`, `AsBool`, `AsDate`, `Math`
//!
//! Each submodule adds helper methods to [`GremlinCompiler`]; the dispatcher
//! below preserves the original match exactly.

mod case_trim;
mod concat;
mod conversion;
mod inspect;
mod mutate;

use super::super::compile_from_context::CompileFromContext;
use super::super::{CompileError, GremlinCompiler};
use super::StepFlow;
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    pub(super) fn compile_strings_step(
        &mut self,
        step: &GremlinStep,
        ctx: &mut CompileFromContext,
    ) -> Result<StepFlow, CompileError> {
        match step {
            GremlinStep::ToLower => self.compile_str_to_lower(ctx)?,
            GremlinStep::ToUpper => self.compile_str_to_upper(ctx)?,
            GremlinStep::Trim => self.compile_str_trim(ctx)?,
            GremlinStep::LTrim => self.compile_str_ltrim(ctx)?,
            GremlinStep::RTrim => self.compile_str_rtrim(ctx)?,

            GremlinStep::Length => self.compile_str_length(ctx)?,
            GremlinStep::Reverse => self.compile_str_reverse(ctx)?,
            GremlinStep::TypeOf => self.compile_str_typeof(ctx)?,

            GremlinStep::Replace(old, new) => self.compile_str_replace(old, new, ctx)?,
            GremlinStep::Split(sep) => self.compile_str_split(sep, ctx)?,
            GremlinStep::Substring(start, len) => self.compile_str_substring(*start, *len, ctx)?,
            GremlinStep::SubstringLocal(start, len) => {
                self.compile_str_substring_local(*start, *len, ctx)?
            }

            GremlinStep::Concat(traversals) => self.compile_str_concat(traversals, ctx)?,
            GremlinStep::Conjoin(sep) => self.compile_str_conjoin(sep, ctx)?,
            GremlinStep::Format(pattern) => self.compile_str_format(pattern, ctx)?,
            GremlinStep::StringOpLocal(fn_name) => self.compile_str_op_local(fn_name, ctx)?,

            GremlinStep::AsString => self.compile_str_as_string(ctx)?,
            GremlinStep::AsStringLocal => self.compile_str_as_string_local(ctx)?,
            GremlinStep::AsNumber => self.compile_str_as_number(ctx)?,
            GremlinStep::AsNumberTyped(ref target_type) => {
                self.compile_str_as_number_typed(target_type, ctx)?
            }
            GremlinStep::AsBool => self.compile_str_as_bool(ctx)?,
            GremlinStep::Math(expr_str) => self.compile_str_math(expr_str, ctx),
            GremlinStep::AsDate => self.compile_str_as_date(ctx)?,
            _ => unreachable!(
                "compile_strings_step received non-strings variant: {:?}",
                step
            ),
        }
        Ok(StepFlow::Proceed)
    }
}
