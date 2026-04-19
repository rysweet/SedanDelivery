// Outcome mapping for graph write-lock renewal and release operations.
//
// These helpers translate the boolean/error results returned by the underlying
// `lock` module into typed `StorageError` values, while emitting the
// appropriate Prometheus counters and tracing logs.

use cypher_core::error::StorageError;

use crate::error_metrics::{
    record_write_lock_backend_failure, record_write_lock_conflict_outcome,
    REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL, REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL,
    REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL, REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL,
};

pub(crate) fn map_lock_renewal_result(
    graph: &str,
    renewal: Result<bool, StorageError>,
) -> Result<(), StorageError> {
    match renewal {
        Ok(true) => {
            metrics::counter!(REDIS_WRITE_LOCK_RENEW_SUCCESSES_TOTAL).increment(1);
            Ok(())
        }
        Ok(false) => {
            metrics::counter!(REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL).increment(1);
            record_write_lock_conflict_outcome("renew");
            tracing::warn!(
                graph = %graph,
                "graph write lock expired or changed owner before renewal"
            );
            Err(StorageError::TransactionConflict)
        }
        Err(err) => {
            metrics::counter!(REDIS_WRITE_LOCK_RENEW_FAILURES_TOTAL).increment(1);
            record_write_lock_backend_failure("renew");
            tracing::warn!(
                graph = %graph,
                error = %err,
                "graph write lock renewal failed"
            );
            Err(err)
        }
    }
}

pub(crate) fn map_lock_release_result<T>(
    graph: &str,
    release: Result<bool, StorageError>,
    result: Result<T, StorageError>,
) -> Result<T, StorageError> {
    match release {
        Ok(true) => {
            metrics::counter!(REDIS_WRITE_LOCK_RELEASE_SUCCESSES_TOTAL).increment(1);
            result
        }
        Ok(false) => {
            metrics::counter!(REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL).increment(1);
            tracing::warn!(
                graph = %graph,
                "graph write lock expired or changed owner before release"
            );
            let result = match result {
                Ok(_) => Err(StorageError::TransactionConflict),
                Err(err) => Err(err),
            };
            if matches!(&result, Err(StorageError::TransactionConflict)) {
                record_write_lock_conflict_outcome("release");
            }
            result
        }
        Err(err) => {
            metrics::counter!(REDIS_WRITE_LOCK_RELEASE_FAILURES_TOTAL).increment(1);
            record_write_lock_backend_failure("release");
            tracing::warn!(
                graph = %graph,
                error = %err,
                "graph write lock release failed"
            );
            match result {
                Ok(_) => Err(err),
                Err(work_err) => Err(work_err),
            }
        }
    }
}
