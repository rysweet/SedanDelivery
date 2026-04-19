/// SedanDB client -- top-level connection wrapper around the redis crate.
use redis::Client;

use crate::error::SedanError;
use crate::graph::Graph;

/// Connection to a SedanDelivery server (backed by the redis crate).
///
/// # Example
///
/// ```no_run
/// use sedan_client::SedanDB;
///
/// let db = SedanDB::new("redis://localhost:6379").unwrap();
/// let mut g = db.select_graph("social").unwrap();
/// let result = g.query("MATCH (n) RETURN n.name").unwrap();
/// ```
pub struct SedanDB {
    client: Client,
}

impl SedanDB {
    /// Create a new SedanDB instance from a Redis URL.
    ///
    /// The URL should be in the form `redis://[password@]host:port[/db]`.
    pub fn new(url: &str) -> Result<Self, SedanError> {
        let client = Client::open(url)?;
        Ok(Self { client })
    }

    /// Return a Graph handle for the named graph.
    ///
    /// Each call opens a new connection from the underlying client.
    pub fn select_graph(&self, name: &str) -> Result<Graph, SedanError> {
        let conn = self.client.get_connection()?;
        Ok(Graph::new(conn, name.to_string()))
    }

    /// Return the names of all graphs on the server.
    pub fn list_graphs(&self) -> Result<Vec<String>, SedanError> {
        let mut conn = self.client.get_connection()?;
        let value: redis::Value = redis::cmd("GQL.LIST-GRAPHS").query(&mut conn)?;
        match value {
            redis::Value::Array(items) => {
                let mut names = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        redis::Value::BulkString(bytes) => {
                            names.push(String::from_utf8_lossy(&bytes).to_string());
                        }
                        redis::Value::SimpleString(s) => {
                            names.push(s);
                        }
                        _ => {
                            return Err(SedanError::Protocol(format!(
                                "Expected string in graph list, got {:?}",
                                item
                            )));
                        }
                    }
                }
                Ok(names)
            }
            redis::Value::Nil => Ok(vec![]),
            _ => Err(SedanError::Protocol(format!(
                "Expected Array from GQL.LIST-GRAPHS, got {:?}",
                value
            ))),
        }
    }
}
