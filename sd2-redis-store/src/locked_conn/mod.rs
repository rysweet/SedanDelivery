// Held write-lock state plus the connection wrapper that owns it.
//
// `LockedRedisConn` represents a connection on which one or more graph write
// locks are held. The closure-based `with_write_lock` API in `store.rs` hands
// out a `&mut LockedRedisConn` and ensures the locks are released (and the
// hold-time histogram is recorded) whether the closure returns Ok or Err.

mod conn;
mod outcomes;

#[cfg(test)]
mod tests;

pub(crate) use conn::LockedRedisConn;
pub(crate) use outcomes::map_lock_release_result;
