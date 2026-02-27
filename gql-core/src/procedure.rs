//! Procedure registry for CALL clause support.
//!
//! Provides [`ProcedureRegistry`] for registering and looking up procedure
//! definitions, and [`ProcedureDefinition`] describing a procedure's signature
//! and data.

use std::collections::HashMap;

use crate::types::Value;

/// A single parameter in a procedure signature.
#[derive(Debug, Clone)]
pub struct ProcedureParam {
    pub name: String,
    pub type_name: String, // e.g. "STRING?", "INTEGER?", "NUMBER?", "FLOAT?"
}

/// A procedure definition: name, input parameters, output columns, and data rows.
#[derive(Debug, Clone)]
pub struct ProcedureDefinition {
    pub name: String,
    pub input_params: Vec<ProcedureParam>,
    pub output_columns: Vec<ProcedureParam>,
    /// Each row contains values for all columns (inputs then outputs).
    pub data: Vec<Vec<Value>>,
}

/// Registry mapping procedure names to their definitions.
#[derive(Debug, Clone, Default)]
pub struct ProcedureRegistry {
    procedures: HashMap<String, ProcedureDefinition>,
}

impl ProcedureRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, proc_def: ProcedureDefinition) {
        self.procedures.insert(proc_def.name.clone(), proc_def);
    }

    pub fn get(&self, name: &str) -> Option<&ProcedureDefinition> {
        self.procedures.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.procedures.contains_key(name)
    }
}

impl ProcedureDefinition {
    /// Execute the procedure with the given evaluated arguments.
    /// Returns rows of output columns matching the input arguments.
    pub fn execute(&self, args: &[Value]) -> Vec<HashMap<String, Value>> {
        let input_count = self.input_params.len();
        let mut results = Vec::new();

        for data_row in &self.data {
            let mut matches = true;
            for i in 0..input_count {
                if i < args.len() && i < data_row.len() {
                    if !values_match_for_lookup(&args[i], &data_row[i]) {
                        matches = false;
                        break;
                    }
                }
            }

            if matches {
                let mut row = HashMap::new();
                for (j, col) in self.output_columns.iter().enumerate() {
                    let val_idx = input_count + j;
                    if val_idx < data_row.len() {
                        row.insert(col.name.clone(), data_row[val_idx].clone());
                    } else {
                        row.insert(col.name.clone(), Value::Null);
                    }
                }
                results.push(row);
            }
        }

        results
    }

    /// Check whether a value is type-compatible with a parameter.
    pub fn check_arg_type(&self, param_idx: usize, value: &Value) -> bool {
        if param_idx >= self.input_params.len() {
            return false;
        }
        let type_name = &self.input_params[param_idx].type_name;
        is_type_compatible(type_name, value)
    }
}

/// Check if two values match for procedure data lookup.
fn values_match_for_lookup(arg: &Value, data: &Value) -> bool {
    match (arg, data) {
        (Value::Null, Value::Null) => true,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => (a - b).abs() < 1e-10,
        (Value::Int(a), Value::Float(b)) => (*a as f64 - b).abs() < 1e-10,
        (Value::Float(a), Value::Int(b)) => (a - *b as f64).abs() < 1e-10,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        _ => false,
    }
}

/// Check if a value is compatible with a declared type.
fn is_type_compatible(type_name: &str, value: &Value) -> bool {
    if value.is_null() {
        return true; // null is compatible with any nullable type
    }
    match type_name.trim_end_matches('?') {
        "STRING" => matches!(value, Value::Str(_)),
        "INTEGER" => matches!(value, Value::Int(_)),
        "FLOAT" => matches!(value, Value::Int(_) | Value::Float(_)),
        "NUMBER" => matches!(value, Value::Int(_) | Value::Float(_)),
        "BOOLEAN" => matches!(value, Value::Bool(_)),
        "ANY" => true,
        _ => true, // unknown types accept anything
    }
}
