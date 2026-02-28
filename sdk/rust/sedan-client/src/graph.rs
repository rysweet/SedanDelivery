/// Graph handle -- sends GQL commands and parses wire-format responses.

use redis::Connection;

use crate::error::SedanError;
use crate::result::{parse_mutate_response, parse_query_response, QueryResult};
use crate::types::{substitute_params, SedanValue};

/// Handle to a single named graph on a SedanDelivery server.
///
/// Obtained via `SedanDB::select_graph(name)`. All Cypher execution goes
/// through `query` (reads) and `mutate` (writes).
pub struct Graph {
    conn: Connection,
    name: String,
}

impl Graph {
    /// Create a new Graph handle.
    ///
    /// This is not typically called directly; use `SedanDB::select_graph` instead.
    pub fn new(conn: Connection, name: String) -> Self {
        Self { conn, name }
    }

    /// Return the name of this graph.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Execute a read-only Cypher query.
    ///
    /// Returns a `QueryResult` containing decoded records.
    pub fn query(&mut self, cypher: &str) -> Result<QueryResult, SedanError> {
        let value: redis::Value =
            redis::cmd("GQL.QUERY")
                .arg(&self.name)
                .arg(cypher)
                .query(&mut self.conn)?;
        parse_query_response(value)
    }

    /// Execute a read-only Cypher query with parameter substitution.
    ///
    /// Parameters in the query string (`$name`) are replaced with
    /// Cypher-escaped values from `params`.
    pub fn query_with_params(
        &mut self,
        cypher: &str,
        params: &[(&str, SedanValue)],
    ) -> Result<QueryResult, SedanError> {
        let resolved = substitute_params(cypher, params);
        self.query(&resolved)
    }

    /// Execute a mutating Cypher query.
    ///
    /// Returns a `QueryResult` containing decoded records and mutation stats.
    pub fn mutate(&mut self, cypher: &str) -> Result<QueryResult, SedanError> {
        let value: redis::Value =
            redis::cmd("GQL.MUTATE")
                .arg(&self.name)
                .arg(cypher)
                .query(&mut self.conn)?;
        parse_mutate_response(value)
    }

    /// Execute a mutating Cypher query with parameter substitution.
    pub fn mutate_with_params(
        &mut self,
        cypher: &str,
        params: &[(&str, SedanValue)],
    ) -> Result<QueryResult, SedanError> {
        let resolved = substitute_params(cypher, params);
        self.mutate(&resolved)
    }

    /// Create this graph on the server.
    pub fn create(&mut self) -> Result<(), SedanError> {
        redis::cmd("GQL.CREATE-GRAPH")
            .arg(&self.name)
            .query::<()>(&mut self.conn)?;
        Ok(())
    }

    /// Delete this graph from the server.
    pub fn delete(&mut self) -> Result<(), SedanError> {
        redis::cmd("GQL.DELETE-GRAPH")
            .arg(&self.name)
            .query::<()>(&mut self.conn)?;
        Ok(())
    }

    /// Return the physical execution plan for a Cypher query.
    pub fn explain(&mut self, cypher: &str) -> Result<String, SedanError> {
        let value: redis::Value =
            redis::cmd("GQL.EXPLAIN")
                .arg(&self.name)
                .arg(cypher)
                .query(&mut self.conn)?;
        match value {
            redis::Value::BulkString(bytes) => {
                Ok(String::from_utf8_lossy(&bytes).to_string())
            }
            redis::Value::SimpleString(s) => Ok(s),
            _ => Err(SedanError::Protocol(format!(
                "Expected string from GQL.EXPLAIN, got {:?}",
                value
            ))),
        }
    }
}
