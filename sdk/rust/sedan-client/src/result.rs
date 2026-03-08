/// Query result types: QueryResult, Record, and MutationStats.

use std::ops::Index;

use crate::error::SedanError;
use crate::types::decode_value;

/// Statistics returned by a GQL.MUTATE command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MutationStats {
    pub nodes_created: u64,
    pub nodes_deleted: u64,
    pub edges_created: u64,
    pub edges_deleted: u64,
    pub properties_set: u64,
    pub labels_added: u64,
}

impl MutationStats {
    /// Parse a stats string from the GQL.MUTATE wire format.
    ///
    /// Expected format: `nodes_created=1 nodes_deleted=0 edges_created=2 ...`
    pub fn parse(s: &str) -> Self {
        let mut stats = Self::default();
        for part in s.split_whitespace() {
            if let Some((key, val_str)) = part.split_once('=') {
                match val_str.parse::<u64>() {
                    Ok(val) => match key {
                        "nodes_created" => stats.nodes_created = val,
                        "nodes_deleted" => stats.nodes_deleted = val,
                        "edges_created" => stats.edges_created = val,
                        "edges_deleted" => stats.edges_deleted = val,
                        "properties_set" => stats.properties_set = val,
                        "labels_added" => stats.labels_added = val,
                        _ => {
                            eprintln!(
                                "MutationStats::parse: unknown key '{key}' with value '{val_str}'"
                            );
                        }
                    },
                    Err(e) => {
                        eprintln!(
                            "MutationStats::parse: failed to parse value '{val_str}' for key '{key}': {e}"
                        );
                    }
                }
            }
        }
        stats
    }
}

/// A single result row, providing dict-like access to column values.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    data: serde_json::Map<String, serde_json::Value>,
}

impl Record {
    /// Create a new Record from a JSON map.
    pub fn new(data: serde_json::Map<String, serde_json::Value>) -> Self {
        Self { data }
    }

    /// Get a reference to a value by key, or `None` if the key is absent.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.data.get(key)
    }

    /// Get a string value by key.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(|v| v.as_str())
    }

    /// Get an i64 value by key.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.data.get(key).and_then(|v| v.as_i64())
    }

    /// Get an f64 value by key.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.data.get(key).and_then(|v| v.as_f64())
    }

    /// Get a bool value by key.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.data.get(key).and_then(|v| v.as_bool())
    }

    /// Return an iterator over the column names in this record.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.data.keys()
    }

    /// Return a reference to the underlying data map.
    pub fn data(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.data
    }
}

/// Index access: `record["name"]`
///
/// Panics if the key is not found.
impl Index<&str> for Record {
    type Output = serde_json::Value;

    fn index(&self, key: &str) -> &Self::Output {
        &self.data[key]
    }
}

/// Result of a GQL.QUERY or GQL.MUTATE command.
///
/// Iterable over `Record` objects and supports index access and `len()`.
#[derive(Debug, Clone)]
pub struct QueryResult {
    records: Vec<Record>,
    stats: MutationStats,
}

impl QueryResult {
    /// Create a new QueryResult.
    pub fn new(records: Vec<Record>, stats: MutationStats) -> Self {
        Self { records, stats }
    }

    /// Return a slice of all records.
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Return a reference to the mutation stats.
    pub fn stats(&self) -> &MutationStats {
        &self.stats
    }

    /// Return an iterator over the records.
    pub fn iter(&self) -> impl Iterator<Item = &Record> {
        self.records.iter()
    }

    /// Return the number of records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Return true if there are no records.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Index access: `result[0]`
impl Index<usize> for QueryResult {
    type Output = Record;

    fn index(&self, index: usize) -> &Self::Output {
        &self.records[index]
    }
}

/// IntoIterator for for-loop support.
impl IntoIterator for QueryResult {
    type Item = Record;
    type IntoIter = std::vec::IntoIter<Record>;

    fn into_iter(self) -> Self::IntoIter {
        self.records.into_iter()
    }
}

impl<'a> IntoIterator for &'a QueryResult {
    type Item = &'a Record;
    type IntoIter = std::slice::Iter<'a, Record>;

    fn into_iter(self) -> Self::IntoIter {
        self.records.iter()
    }
}

/// Parse the RESP value returned by GQL.QUERY into a QueryResult.
///
/// GQL.QUERY returns an Array of BulkStrings, each being a JSON object.
pub fn parse_query_response(value: redis::Value) -> Result<QueryResult, SedanError> {
    match value {
        redis::Value::Array(items) => {
            let mut records = Vec::with_capacity(items.len());
            for item in &items {
                match item {
                    redis::Value::BulkString(bytes) => {
                        let json: serde_json::Map<String, serde_json::Value> =
                            serde_json::from_slice(bytes)?;
                        let decoded: serde_json::Map<String, serde_json::Value> = json
                            .into_iter()
                            .map(|(k, v)| (k, decode_value(&v)))
                            .collect();
                        records.push(Record::new(decoded));
                    }
                    redis::Value::SimpleString(s) => {
                        let json: serde_json::Map<String, serde_json::Value> =
                            serde_json::from_str(s)?;
                        let decoded: serde_json::Map<String, serde_json::Value> = json
                            .into_iter()
                            .map(|(k, v)| (k, decode_value(&v)))
                            .collect();
                        records.push(Record::new(decoded));
                    }
                    _ => {
                        return Err(SedanError::Protocol(format!(
                            "Expected BulkString in query response, got {:?}",
                            item
                        )));
                    }
                }
            }
            Ok(QueryResult::new(records, MutationStats::default()))
        }
        redis::Value::Nil => Ok(QueryResult::new(vec![], MutationStats::default())),
        _ => Err(SedanError::Protocol(format!(
            "Expected Array or Nil from GQL.QUERY, got {:?}",
            value
        ))),
    }
}

/// Parse the RESP value returned by GQL.MUTATE into a QueryResult.
///
/// Wire format: `[Array-of-JSON-rows, BulkString stats]`.
pub fn parse_mutate_response(value: redis::Value) -> Result<QueryResult, SedanError> {
    match value {
        redis::Value::Array(ref items) if items.len() == 2 => {
            // First element: array of JSON rows (or empty array).
            let records = match &items[0] {
                redis::Value::Array(rows) => {
                    let mut recs = Vec::with_capacity(rows.len());
                    for row in rows {
                        match row {
                            redis::Value::BulkString(bytes) => {
                                let json: serde_json::Map<String, serde_json::Value> =
                                    serde_json::from_slice(bytes)?;
                                let decoded: serde_json::Map<String, serde_json::Value> = json
                                    .into_iter()
                                    .map(|(k, v)| (k, decode_value(&v)))
                                    .collect();
                                recs.push(Record::new(decoded));
                            }
                            redis::Value::SimpleString(s) => {
                                let json: serde_json::Map<String, serde_json::Value> =
                                    serde_json::from_str(s)?;
                                let decoded: serde_json::Map<String, serde_json::Value> = json
                                    .into_iter()
                                    .map(|(k, v)| (k, decode_value(&v)))
                                    .collect();
                                recs.push(Record::new(decoded));
                            }
                            _ => {
                                return Err(SedanError::Protocol(format!(
                                    "Expected BulkString in mutate row, got {:?}",
                                    row
                                )));
                            }
                        }
                    }
                    recs
                }
                redis::Value::Nil => vec![],
                _ => {
                    return Err(SedanError::Protocol(format!(
                        "Expected Array for mutate rows, got {:?}",
                        items[0]
                    )));
                }
            };

            // Second element: stats string.
            let stats = match &items[1] {
                redis::Value::BulkString(bytes) => {
                    let s = String::from_utf8_lossy(bytes);
                    MutationStats::parse(&s)
                }
                redis::Value::SimpleString(s) => MutationStats::parse(s),
                _ => {
                    return Err(SedanError::Protocol(format!(
                        "Expected BulkString for mutate stats, got {:?}",
                        items[1]
                    )));
                }
            };

            Ok(QueryResult::new(records, stats))
        }
        redis::Value::Nil => Ok(QueryResult::new(vec![], MutationStats::default())),
        _ => Err(SedanError::Protocol(format!(
            "Expected [rows, stats] from GQL.MUTATE, got {:?}",
            value
        ))),
    }
}
