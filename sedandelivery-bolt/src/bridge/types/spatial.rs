//! Spatial Cypher → Bolt encoders (Point2D, Point3D).

use cypher_core::types::value::{Crs, GeoPoint};

use crate::packstream::value::BoltValue;
use crate::protocol::tags;

/// Point2D or Point3D encoding based on the presence of a z coordinate.
pub(super) fn point_to_bolt(p: &GeoPoint) -> BoltValue {
    let srid: i64 = match p.crs {
        Crs::Wgs84 => 4326,
        Crs::Wgs843d => 4979,
        Crs::Cartesian => 7203,
        Crs::Cartesian3d => 9157,
    };
    if let Some(z) = p.z {
        BoltValue::Structure {
            tag: tags::TAG_POINT_3D,
            fields: vec![
                BoltValue::Int(srid),
                BoltValue::Float(p.x),
                BoltValue::Float(p.y),
                BoltValue::Float(z),
            ],
        }
    } else {
        BoltValue::Structure {
            tag: tags::TAG_POINT_2D,
            fields: vec![
                BoltValue::Int(srid),
                BoltValue::Float(p.x),
                BoltValue::Float(p.y),
            ],
        }
    }
}
