//! Arithmetic operators: `+`, `-`, `*`, `/`, `%`, `^`.
//!
//! Includes string/list concatenation (sharing the `+` arm) and
//! temporal/duration arithmetic. Each operator lives in its own submodule
//! to keep individual files focused and small.

mod add;
mod div_mod;
mod mul;
mod sub;

#[cfg(test)]
mod tests;

pub(super) use add::add;
pub(super) use div_mod::{div, modulo, pow};
pub(super) use mul::mul;
pub(super) use sub::sub;
