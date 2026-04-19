//! time() and localtime() constructors.

mod local;
mod zoned;

pub(crate) use self::local::{builtin_temporal_realtime_localtime, eval_localtime_constructor};
pub(crate) use self::zoned::{builtin_temporal_realtime_time, eval_time_constructor};
