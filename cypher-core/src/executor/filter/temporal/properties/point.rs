//! Property access for `Value::Point` (spatial).

use crate::error::GqlError;
use crate::types::value::{Crs, GeoPoint};
use crate::types::Value;

pub(super) fn point_property(p: &GeoPoint, key: &str) -> Result<Value, GqlError> {
    match key {
        "x" => Ok(Value::Float(p.x)),
        "y" => Ok(Value::Float(p.y)),
        "z" => match p.z {
            Some(z) => Ok(Value::Float(z)),
            None => Ok(Value::Null),
        },
        "srid" => {
            let srid = match p.crs {
                Crs::Wgs84 => 4326,
                Crs::Wgs843d => 4979,
                Crs::Cartesian => 7203,
                Crs::Cartesian3d => 9157,
            };
            Ok(Value::Int(srid))
        }
        "crs" => {
            let name = match p.crs {
                Crs::Wgs84 => "wgs-84",
                Crs::Wgs843d => "wgs-84-3d",
                Crs::Cartesian => "cartesian",
                Crs::Cartesian3d => "cartesian-3d",
            };
            Ok(Value::Str(name.into()))
        }
        // For WGS84 points, also expose longitude/latitude/height aliases.
        "longitude" if matches!(p.crs, Crs::Wgs84 | Crs::Wgs843d) => Ok(Value::Float(p.x)),
        "latitude" if matches!(p.crs, Crs::Wgs84 | Crs::Wgs843d) => Ok(Value::Float(p.y)),
        "height" if matches!(p.crs, Crs::Wgs843d) => match p.z {
            Some(z) => Ok(Value::Float(z)),
            None => Ok(Value::Null),
        },
        _ => Ok(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::super::temporal_property_access;
    use crate::types::value::{Crs, GeoPoint};
    use crate::types::Value;

    #[test]
    fn point_props() {
        let p = Value::Point(GeoPoint {
            x: 1.0,
            y: 2.0,
            z: None,
            crs: Crs::Cartesian,
        });
        assert_eq!(
            temporal_property_access(&p, "x").unwrap(),
            Value::Float(1.0)
        );
        assert_eq!(
            temporal_property_access(&p, "srid").unwrap(),
            Value::Int(7203)
        );
    }

    #[test]
    fn point_all_keys_wgs84_3d() {
        let p = Value::Point(GeoPoint {
            x: 10.0,
            y: 20.0,
            z: Some(100.0),
            crs: Crs::Wgs843d,
        });
        for k in ["x", "y", "z", "longitude", "latitude", "height"] {
            let _ = temporal_property_access(&p, k).unwrap();
        }
        assert!(matches!(
            temporal_property_access(&p, "srid").unwrap(),
            Value::Int(_)
        ));
        if let Value::Str(s) = temporal_property_access(&p, "crs").unwrap() {
            assert_eq!(&*s, "wgs-84-3d");
        } else {
            panic!()
        }
        assert_eq!(temporal_property_access(&p, "nope").unwrap(), Value::Null);
    }

    #[test]
    fn point_all_keys_cartesian2d_no_z() {
        let p = Value::Point(GeoPoint {
            x: 1.0,
            y: 2.0,
            z: None,
            crs: Crs::Cartesian,
        });
        assert_eq!(temporal_property_access(&p, "z").unwrap(), Value::Null);
        if let Value::Str(s) = temporal_property_access(&p, "crs").unwrap() {
            assert_eq!(&*s, "cartesian");
        } else {
            panic!()
        }
        assert_eq!(
            temporal_property_access(&p, "longitude").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn point_crs_variants() {
        for (crs, want_srid, want_name) in [
            (Crs::Wgs84, 4326, "wgs-84"),
            (Crs::Wgs843d, 4979, "wgs-84-3d"),
            (Crs::Cartesian, 7203, "cartesian"),
            (Crs::Cartesian3d, 9157, "cartesian-3d"),
        ] {
            let p = Value::Point(GeoPoint {
                x: 0.0,
                y: 0.0,
                z: None,
                crs,
            });
            assert_eq!(
                temporal_property_access(&p, "srid").unwrap(),
                Value::Int(want_srid)
            );
            if let Value::Str(s) = temporal_property_access(&p, "crs").unwrap() {
                assert_eq!(&*s, want_name);
            } else {
                panic!()
            }
        }
    }
}
