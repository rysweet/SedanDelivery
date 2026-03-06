//! Point constructor and spatial reference handling.

use crate::error::{ExecutionError, GqlError};
use crate::types::value::{Crs, GeoPoint};
use crate::types::Value;

/// Evaluate the `point()` constructor function.
///
/// Accepts a single Map argument with coordinate keys:
/// - `x`/`y`[/`z`] for Cartesian points (SRID 7203 / 9157)
/// - `longitude`/`latitude`[/`height`] for WGS84 points (SRID 4326 / 4979)
/// - Optional `srid` to override the default CRS inference
pub(super) fn eval_point_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "point() requires exactly 1 argument (a map)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }

    let m = match &vals[0] {
        Value::Null => return Ok(Value::Null),
        Value::Map(m) => m,
        other => {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "Map argument for point()".to_string(),
                actual: other.type_name().to_string(),
            }));
        }
    };

    // Helper: extract an f64 from the map (accepts Int or Float)
    let get_f64 = |key: &str| -> Option<f64> {
        match m.get(key) {
            Some(Value::Float(f)) => Some(*f),
            Some(Value::Int(i)) => Some(*i as f64),
            _ => None,
        }
    };

    // Extract explicit SRID if provided
    let explicit_srid: Option<i64> = match m.get("srid") {
        Some(Value::Int(n)) => Some(*n),
        Some(Value::Float(f)) if f.is_finite() => Some(*f as i64),
        Some(Value::Float(_)) => None,
        _ => None,
    };

    // Determine coordinates and CRS
    let has_x = m.contains_key("x");
    let has_y = m.contains_key("y");
    let has_z = m.contains_key("z");
    let has_lon = m.contains_key("longitude");
    let has_lat = m.contains_key("latitude");
    let has_height = m.contains_key("height");

    let (x, y, z, inferred_crs) = if has_lon && has_lat {
        let lon = get_f64("longitude").unwrap_or(0.0);
        let lat = get_f64("latitude").unwrap_or(0.0);
        if has_height {
            let h = get_f64("height").unwrap_or(0.0);
            (lon, lat, Some(h), Crs::Wgs843d)
        } else {
            (lon, lat, None, Crs::Wgs84)
        }
    } else if has_x && has_y {
        let xv = get_f64("x").unwrap_or(0.0);
        let yv = get_f64("y").unwrap_or(0.0);
        if has_z {
            let zv = get_f64("z").unwrap_or(0.0);
            (xv, yv, Some(zv), Crs::Cartesian3d)
        } else {
            (xv, yv, None, Crs::Cartesian)
        }
    } else {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Map with x/y or longitude/latitude keys".to_string(),
            actual: "incomplete coordinate keys".to_string(),
        }));
    };

    // Apply explicit SRID override
    let crs = match explicit_srid {
        Some(4326) => Crs::Wgs84,
        Some(4979) => Crs::Wgs843d,
        Some(7203) => Crs::Cartesian,
        Some(9157) => Crs::Cartesian3d,
        Some(other) => {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "valid SRID (4326, 4979, 7203, 9157)".to_string(),
                actual: format!("SRID {other}"),
            }));
        }
        None => inferred_crs,
    };

    Ok(Value::Point(GeoPoint { x, y, z, crs }))
}
