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
