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
                if i < args.len()
                    && i < data_row.len()
                    && !values_match_for_lookup(&args[i], &data_row[i])
                {
                    matches = false;
                    break;
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

// ── Built-in procedure support ────────────────────────────────────────────────

/// Names of procedures that are handled dynamically by the executor
/// (they query the store at runtime rather than using static registry data).
pub const BUILTIN_PROCEDURES: &[&str] = &[
    "db.labels",
    "db.relationshipTypes",
    "db.propertyKeys",
    "dbms.components",
    "db.schema.visualization",
];

/// Check whether a procedure name is a built-in that the executor handles dynamically.
pub fn is_builtin_procedure(name: &str) -> bool {
    BUILTIN_PROCEDURES.contains(&name)
}

/// Metadata for built-in procedures: output column definitions.
/// These have no input parameters.
pub fn builtin_output_columns(name: &str) -> Option<Vec<ProcedureParam>> {
    match name {
        "db.labels" => Some(vec![ProcedureParam {
            name: "label".to_string(),
            type_name: "STRING?".to_string(),
        }]),
        "db.relationshipTypes" => Some(vec![ProcedureParam {
            name: "relationshipType".to_string(),
            type_name: "STRING?".to_string(),
        }]),
        "db.propertyKeys" => Some(vec![ProcedureParam {
            name: "propertyKey".to_string(),
            type_name: "STRING?".to_string(),
        }]),
        "dbms.components" => Some(vec![
            ProcedureParam {
                name: "name".to_string(),
                type_name: "STRING?".to_string(),
            },
            ProcedureParam {
                name: "versions".to_string(),
                type_name: "ANY?".to_string(),
            },
            ProcedureParam {
                name: "edition".to_string(),
                type_name: "STRING?".to_string(),
            },
        ]),
        "db.schema.visualization" => Some(vec![
            ProcedureParam {
                name: "nodes".to_string(),
                type_name: "ANY?".to_string(),
            },
            ProcedureParam {
                name: "relationships".to_string(),
                type_name: "ANY?".to_string(),
            },
        ]),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Value;

    fn make_double_proc() -> ProcedureDefinition {
        ProcedureDefinition {
            name: "double".to_string(),
            input_params: vec![ProcedureParam {
                name: "n".to_string(),
                type_name: "INTEGER?".to_string(),
            }],
            output_columns: vec![ProcedureParam {
                name: "doubled".to_string(),
                type_name: "INTEGER?".to_string(),
            }],
            data: vec![
                vec![Value::Int(1), Value::Int(2)],
                vec![Value::Int(2), Value::Int(4)],
                vec![Value::Int(3), Value::Int(6)],
            ],
        }
    }

    #[test]
    fn registry_register_and_get() {
        let mut reg = ProcedureRegistry::new();
        reg.register(make_double_proc());
        let got = reg.get("double").expect("procedure should be registered");
        assert_eq!(got.name, "double");
        assert_eq!(got.input_params.len(), 1);
        assert_eq!(got.output_columns.len(), 1);
    }

    #[test]
    fn registry_contains_after_register() {
        let mut reg = ProcedureRegistry::new();
        assert!(!reg.contains("double"));
        reg.register(make_double_proc());
        assert!(reg.contains("double"));
    }

    #[test]
    fn registry_get_unknown_returns_none() {
        let reg = ProcedureRegistry::new();
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn procedure_execute_filters_by_input_arg() {
        let proc = make_double_proc();
        // Execute with n=2 should return the row where input=2, output=4.
        let results = proc.execute(&[Value::Int(2)]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["doubled"], Value::Int(4));

        // Execute with n=99 (no matching row) should return empty.
        let empty = proc.execute(&[Value::Int(99)]);
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn check_arg_type_validates_declared_type() {
        let proc = make_double_proc();
        // INTEGER? accepts Int values.
        assert!(proc.check_arg_type(0, &Value::Int(5)));
        // INTEGER? accepts Null (nullable).
        assert!(proc.check_arg_type(0, &Value::Null));
        // INTEGER? rejects String.
        assert!(!proc.check_arg_type(0, &Value::Str("hello".into())));
        // Out-of-range param index always returns false.
        assert!(!proc.check_arg_type(99, &Value::Int(1)));
    }
}
