//! Spatial Bolt structure decoders.
//!
//! Decodes Bolt Point2D and Point3D structures into cypher-core `Value` types.

use cypher_core::types::value::{Crs, GeoPoint};
use cypher_core::types::Value;

use crate::bridge::temporal::bolt_int;
use crate::packstream::value::BoltValue;

/// Extract an f64 from a BoltValue (accepts both Int and Float).
fn bolt_float(v: &BoltValue) -> Option<f64> {
    match v {
        BoltValue::Float(f) => Some(*f),
        BoltValue::Int(i) => Some(*i as f64),
        _ => None,
    }
}

/// Map an SRID integer to a Crs variant. Returns `None` for unknown SRIDs.
pub(crate) fn srid_to_crs(srid: i64) -> Option<Crs> {
    match srid {
        4326 => Some(Crs::Wgs84),
        4979 => Some(Crs::Wgs843d),
        7203 => Some(Crs::Cartesian),
        9157 => Some(Crs::Cartesian3d),
        _ => None,
    }
}

/// Point2D (tag 0x58): 3 fields -- srid, x, y.
pub(crate) fn decode_point2d(fields: &[BoltValue]) -> Value {
    let srid = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'srid' field in Point2D structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let x = match fields.get(1).and_then(bolt_float) {
        Some(v) => v,
        None => {
            tracing::warn!("bolt_decode: missing or non-numeric 'x' field in Point2D structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let y = match fields.get(2).and_then(bolt_float) {
        Some(v) => v,
        None => {
            tracing::warn!("bolt_decode: missing or non-numeric 'y' field in Point2D structure, fields={fields:?}");
            return Value::Null;
        }
    };

    let crs = match srid_to_crs(srid) {
        Some(c) => c,
        None => {
            tracing::error!(srid, "bolt_decode: unknown SRID in Point2D structure");
            return Value::Null;
        }
    };

    Value::Point(GeoPoint { x, y, z: None, crs })
}

/// Point3D (tag 0x59): 4 fields -- srid, x, y, z.
pub(crate) fn decode_point3d(fields: &[BoltValue]) -> Value {
    let srid = match fields.first().and_then(bolt_int) {
        Some(s) => s,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'srid' field in Point3D structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let x = match fields.get(1).and_then(bolt_float) {
        Some(v) => v,
        None => {
            tracing::warn!("bolt_decode: missing or non-numeric 'x' field in Point3D structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let y = match fields.get(2).and_then(bolt_float) {
        Some(v) => v,
        None => {
            tracing::warn!("bolt_decode: missing or non-numeric 'y' field in Point3D structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let z = fields.get(3).and_then(bolt_float);

    let crs = match srid_to_crs(srid) {
        Some(c) => c,
        None => {
            tracing::error!(srid, "bolt_decode: unknown SRID in Point3D structure");
            return Value::Null;
        }
    };

    Value::Point(GeoPoint { x, y, z, crs })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Point2D tests ───────────────────────────────────────────────────────

    #[test]
    fn decode_point2d_wgs84_from_bolt_structure() {
        let val = decode_point2d(&[
            BoltValue::Int(4326),
            BoltValue::Float(-73.95),
            BoltValue::Float(40.78),
        ]);
        let expected = GeoPoint {
            x: -73.95,
            y: 40.78,
            z: None,
            crs: Crs::Wgs84,
        };
        assert_eq!(val, Value::Point(expected));
    }

    #[test]
    fn decode_point2d_cartesian_from_bolt_structure() {
        let val = decode_point2d(&[
            BoltValue::Int(7203),
            BoltValue::Float(1.5),
            BoltValue::Float(2.5),
        ]);
        let expected = GeoPoint {
            x: 1.5,
            y: 2.5,
            z: None,
            crs: Crs::Cartesian,
        };
        assert_eq!(val, Value::Point(expected));
    }

    #[test]
    fn decode_point2d_missing_srid_returns_null() {
        assert_eq!(decode_point2d(&[]), Value::Null);
    }

    #[test]
    fn decode_point2d_missing_x_returns_null() {
        assert_eq!(decode_point2d(&[BoltValue::Int(7203)]), Value::Null);
    }

    #[test]
    fn decode_point2d_missing_y_returns_null() {
        assert_eq!(
            decode_point2d(&[BoltValue::Int(7203), BoltValue::Float(1.0)]),
            Value::Null
        );
    }

    // ── Point3D tests ───────────────────────────────────────────────────────

    #[test]
    fn decode_point3d_from_bolt_structure() {
        let val = decode_point3d(&[
            BoltValue::Int(4979),
            BoltValue::Float(1.0),
            BoltValue::Float(2.0),
            BoltValue::Float(3.0),
        ]);
        let expected = GeoPoint {
            x: 1.0,
            y: 2.0,
            z: Some(3.0),
            crs: Crs::Wgs843d,
        };
        assert_eq!(val, Value::Point(expected));
    }

    #[test]
    fn decode_point3d_missing_srid_returns_null() {
        assert_eq!(decode_point3d(&[]), Value::Null);
    }

    #[test]
    fn decode_point3d_missing_x_returns_null() {
        assert_eq!(decode_point3d(&[BoltValue::Int(4979)]), Value::Null);
    }

    #[test]
    fn decode_point3d_missing_y_returns_null() {
        assert_eq!(
            decode_point3d(&[BoltValue::Int(4979), BoltValue::Float(1.0)]),
            Value::Null
        );
    }

    #[test]
    fn decode_point3d_missing_z_still_succeeds() {
        let val = decode_point3d(&[
            BoltValue::Int(4979),
            BoltValue::Float(1.0),
            BoltValue::Float(2.0),
        ]);
        if let Value::Point(p) = val {
            assert!(p.z.is_none());
        } else {
            panic!("expected Value::Point, got {val:?}");
        }
    }

    // ── srid_to_crs tests ───────────────────────────────────────────────────

    #[test]
    fn srid_to_crs_known_values() {
        assert_eq!(srid_to_crs(4326), Some(Crs::Wgs84));
        assert_eq!(srid_to_crs(4979), Some(Crs::Wgs843d));
        assert_eq!(srid_to_crs(7203), Some(Crs::Cartesian));
        assert_eq!(srid_to_crs(9157), Some(Crs::Cartesian3d));
    }

    #[test]
    fn srid_to_crs_unknown_returns_none() {
        assert_eq!(srid_to_crs(9999), None);
    }
}
