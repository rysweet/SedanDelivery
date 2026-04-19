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
