//! Element-accessor expression and function handlers split out of `eval`:
//! `Expr::Property`, `Expr::HasLabel`, `Expr::Subscript`, and the function
//! arms `id`, `labels`, `type`, `properties`, `keys`, `propertiesMatch`,
//! `hasLabel`, gremlin `T.label/T.key/T.value`.
//!
//! Submodules group the methods by topic:
//!
//! - [`expressions`] — `Expr::Property`, `Expr::HasLabel`, `Expr::Subscript`
//! - [`labels`] — `labels()` and `hasLabel()` function arms
//! - [`properties`] — `properties()`, `keys()`, `propertiesMatch()` function arms
//! - [`gremlin`] — gremlin `T.label`, `T.key`, `T.value` function arms
//! - [`type_id`] — `type()` and `id()` function arms

mod expressions;
mod gremlin;
mod labels;
mod properties;
mod type_id;
