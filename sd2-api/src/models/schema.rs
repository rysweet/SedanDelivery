//! Schema introspection response models.

use serde::Serialize;

#[derive(Serialize)]
pub struct LabelInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Serialize)]
pub struct RelTypeInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaResponse {
    pub labels: Vec<LabelInfo>,
    pub relationship_types: Vec<RelTypeInfo>,
}
