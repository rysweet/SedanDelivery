//! `asNumber` and typed numeric conversions.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn asnumber(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Int(n)) => Ok(Value::Int(*n)),
        Some(Value::Float(f)) => Ok(Value::Float(*f)),
        Some(Value::Str(s)) => {
            // Try hex first: 0x5
            if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                if let Ok(n) = i64::from_str_radix(hex, 16) {
                    return Ok(Value::Int(n));
                }
            }
            if let Ok(n) = s.parse::<i64>() {
                Ok(Value::Int(n))
            } else if let Ok(f) = s.parse::<f64>() {
                Ok(Value::Float(f))
            } else {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("Can't parse string '{}' as number.", s),
                    actual: format!("'{}'", s),
                }))
            }
        }
        Some(Value::Bool(b)) => Ok(Value::Int(if *b { 1 } else { 0 })),
        Some(Value::DateTime(dt)) => {
            // DateTime → epoch millis
            Ok(Value::Int(dt.timestamp().as_millisecond()))
        }
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Can't parse type ArrayList as number.".into(),
            actual: "list".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn asnumber_typed(vals: Vec<Value>) -> Result<Value, GqlError> {
    // asNumber(GType.INT/LONG/FLOAT/DOUBLE/BYTE/SHORT): convert to specific numeric type
    match (vals.first(), vals.get(1)) {
        (_, Some(Value::Str(t))) if t.as_ref() == "__invalid__" => {
            Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "asNumber() requires a numeric type token".into(),
                actual: "non-numeric type".into(),
            }))
        }
        (Some(Value::Int(n)), Some(Value::Str(t))) if t.as_ref() == "byte" => {
            if *n < i8::MIN as i64 || *n > i8::MAX as i64 {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Can't convert number of type Integer to Byte due to overflow."
                        .into(),
                    actual: n.to_string(),
                }))
            } else {
                Ok(Value::Int(*n))
            }
        }
        (Some(Value::Int(n)), Some(Value::Str(t))) if t.as_ref() == "short" => {
            if *n < i16::MIN as i64 || *n > i16::MAX as i64 {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Can't convert number of type Integer to Short due to overflow."
                        .into(),
                    actual: n.to_string(),
                }))
            } else {
                Ok(Value::Int(*n))
            }
        }
        (Some(Value::Float(f)), Some(Value::Str(t))) if t.as_ref() == "int" => {
            Ok(Value::Int(*f as i64))
        }
        (Some(Value::Int(n)), Some(Value::Str(t))) if t.as_ref() == "float" => {
            Ok(Value::Float(*n as f64))
        }
        (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "int" => {
            if let Ok(n) = s.parse::<i64>() {
                Ok(Value::Int(n))
            } else if let Ok(f) = s.parse::<f64>() {
                Ok(Value::Int(f as i64))
            } else {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("Can't parse string '{}' as integer.", s),
                    actual: format!("'{}'", s),
                }))
            }
        }
        (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "float" => {
            if let Ok(f) = s.parse::<f64>() {
                Ok(Value::Float(f))
            } else {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("Can't parse string '{}' as float.", s),
                    actual: format!("'{}'", s),
                }))
            }
        }
        (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "byte" => {
            if let Ok(n) = s.parse::<i64>() {
                if n < i8::MIN as i64 || n > i8::MAX as i64 {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Can't convert number of type Integer to Byte due to overflow."
                            .into(),
                        actual: n.to_string(),
                    }))
                } else {
                    Ok(Value::Int(n))
                }
            } else {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("Can't parse string '{}' as byte.", s),
                    actual: format!("'{}'", s),
                }))
            }
        }
        (Some(Value::Str(s)), Some(Value::Str(t))) if t.as_ref() == "short" => {
            if let Ok(n) = s.parse::<i64>() {
                if n < i16::MIN as i64 || n > i16::MAX as i64 {
                    Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Can't convert number of type Integer to Short due to overflow."
                            .into(),
                        actual: n.to_string(),
                    }))
                } else {
                    Ok(Value::Int(n))
                }
            } else {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("Can't parse string '{}' as short.", s),
                    actual: format!("'{}'", s),
                }))
            }
        }
        // byte/short for float inputs — truncate if in range
        (Some(Value::Float(f)), Some(Value::Str(t))) if t.as_ref() == "byte" => {
            let n = *f as i64;
            if n < i8::MIN as i64 || n > i8::MAX as i64 {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Can't convert number to Byte due to overflow.".into(),
                    actual: f.to_string(),
                }))
            } else {
                Ok(Value::Int(n))
            }
        }
        (Some(Value::Float(f)), Some(Value::Str(t))) if t.as_ref() == "short" => {
            let n = *f as i64;
            if n < i16::MIN as i64 || n > i16::MAX as i64 {
                Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Can't convert number to Short due to overflow.".into(),
                    actual: f.to_string(),
                }))
            } else {
                Ok(Value::Int(n))
            }
        }
        (Some(val), _) => {
            // Fall through to default asNumber behavior
            match val {
                Value::Int(n) => Ok(Value::Int(*n)),
                Value::Float(f) => Ok(Value::Float(*f)),
                Value::Null => Ok(Value::Null),
                _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Cannot convert to number".into(),
                    actual: val.type_name().to_string(),
                })),
            }
        }
        _ => Ok(Value::Null),
    }
}
