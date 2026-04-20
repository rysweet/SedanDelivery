// manager — `r2d2::ManageConnection` adapters for standalone and cluster Redis.

use redis::cluster::{ClusterClient, ClusterConnection};
use redis::{Client, Connection, ConnectionLike, RedisError};

use super::config::{closed_connection_error, RedisSocketTimeouts};

#[derive(Clone)]
pub(crate) struct TimeoutClientManager {
    client: Client,
    timeouts: RedisSocketTimeouts,
}

impl TimeoutClientManager {
    pub(crate) fn new(client: Client, timeouts: RedisSocketTimeouts) -> Self {
        Self { client, timeouts }
    }
}

impl r2d2::ManageConnection for TimeoutClientManager {
    type Connection = Connection;
    type Error = RedisError;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let conn = self
            .client
            .get_connection_with_timeout(self.timeouts.connect_timeout)?;
        conn.set_read_timeout(Some(self.timeouts.read_timeout))?;
        conn.set_write_timeout(Some(self.timeouts.write_timeout))?;
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        if conn.check_connection() {
            Ok(())
        } else {
            Err(closed_connection_error())
        }
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        !conn.is_open()
    }
}

#[derive(Clone)]
pub(crate) struct TimeoutClusterClientManager {
    client: ClusterClient,
    timeouts: RedisSocketTimeouts,
}

impl TimeoutClusterClientManager {
    pub(crate) fn new(client: ClusterClient, timeouts: RedisSocketTimeouts) -> Self {
        Self { client, timeouts }
    }
}

impl r2d2::ManageConnection for TimeoutClusterClientManager {
    type Connection = ClusterConnection;
    type Error = RedisError;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let conn = self.client.get_connection()?;
        conn.set_read_timeout(Some(self.timeouts.read_timeout))?;
        conn.set_write_timeout(Some(self.timeouts.write_timeout))?;
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        if conn.check_connection() {
            Ok(())
        } else {
            Err(closed_connection_error())
        }
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        !conn.is_open()
    }
}

#[cfg(test)]
mod tests {
    //! Construction-only tests for the r2d2 manager wrappers. The
    //! `connect`/`is_valid`/`has_broken` methods require a live Redis and are
    //! exercised by the live-Redis suite under `tests/`.

    use super::super::config::socket_timeouts;
    use super::*;
    use redis::cluster::ClusterClient;
    use redis::Client;

    #[test]
    fn standalone_manager_clone_preserves_timeouts() {
        let client = Client::open("redis://127.0.0.1:1").expect("client open should succeed");
        let mgr = TimeoutClientManager::new(client, socket_timeouts());
        let cloned = mgr.clone();
        // Both managers must report identical timeout fields.
        assert_eq!(
            mgr.timeouts.connect_timeout,
            cloned.timeouts.connect_timeout
        );
        assert_eq!(mgr.timeouts.read_timeout, cloned.timeouts.read_timeout);
        assert_eq!(mgr.timeouts.write_timeout, cloned.timeouts.write_timeout);
    }

    #[test]
    fn cluster_manager_clone_preserves_timeouts() {
        let client = ClusterClient::new(vec!["redis://127.0.0.1:1", "redis://127.0.0.1:2"])
            .expect("cluster client open should succeed");
        let mgr = TimeoutClusterClientManager::new(client, socket_timeouts());
        let cloned = mgr.clone();
        assert_eq!(
            mgr.timeouts.connect_timeout,
            cloned.timeouts.connect_timeout
        );
        assert_eq!(mgr.timeouts.read_timeout, cloned.timeouts.read_timeout);
        assert_eq!(mgr.timeouts.write_timeout, cloned.timeouts.write_timeout);
    }

    #[test]
    fn standalone_manager_connect_fails_against_unreachable_url() {
        // Exercises `connect()` against an unreachable URL — a fast failure
        // when min_idle=0 isn't possible, so this test runs `connect()`
        // directly to cover the timeout-bound connect path without going
        // through r2d2's pre-fill machinery.
        let client = Client::open("redis://127.0.0.1:1").unwrap();
        let mgr = TimeoutClientManager::new(client, socket_timeouts());
        let err = match r2d2::ManageConnection::connect(&mgr) {
            Err(e) => e,
            Ok(_) => panic!("connect against unreachable URL must fail"),
        };
        // Just assert it produced *some* error; the exact kind depends on the
        // platform and OS-level connect semantics.
        let _ = err.to_string();
    }
}
