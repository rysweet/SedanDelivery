//! Index management for `ModuleRedisStore`.

use cypher_core::error::StorageError;
use cypher_core::storage::IndexDescriptor;

use super::keys::key_indexes;
use super::ModuleRedisStore;

impl ModuleRedisStore {
    pub(super) fn impl_list_indexes(
        &self,
        graph: &str,
    ) -> Result<Vec<IndexDescriptor>, StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_indexes(graph));
            let result = self.call_cmd(ctx, "SMEMBERS", &[&key])?;
            let members = Self::extract_string_array(&result);
            Ok(members
                .into_iter()
                .filter_map(|spec| {
                    spec.split_once('\x00')
                        .map(|(label, property)| IndexDescriptor {
                            label: label.to_string(),
                            property: property.to_string(),
                        })
                })
                .collect())
        })
    }

    pub(super) fn impl_create_index(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_indexes(graph));
            let member = format!("{label}\x00{property}");
            self.call_cmd(ctx, "SADD", &[&key, &member])?;
            Ok(())
        })
    }

    pub(super) fn impl_drop_index(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.with_ctx(|ctx| {
            let key = self.kp(key_indexes(graph));
            let member = format!("{label}\x00{property}");
            self.call_cmd(ctx, "SREM", &[&key, &member])?;
            Ok(())
        })
    }
}
