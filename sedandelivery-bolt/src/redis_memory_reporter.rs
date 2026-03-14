//! Async background task that periodically polls Redis memory metrics.
//!
//! Call [`spawn_redis_memory_reporter`] from `main` after the Redis store is
//! initialised.  The task runs forever until the process exits.

use std::time::Duration;

/// Spawn a background task that polls Redis memory metrics every `interval`.
///
/// Connects to Redis using `url`, polls `INFO memory` + `INFO stats` via
/// [`gql_redis_store::redis_memory_metrics::poll_once`], emits Prometheus
/// gauges for `used_memory` / `maxmemory` / `evicted_keys`, and logs warnings
/// when Redis is under memory pressure (>= 90% of maxmemory) or when eviction
/// has occurred since the last poll.
pub fn spawn_redis_memory_reporter(url: String, interval: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = match redis::Client::open(url.as_str()) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("redis memory reporter: failed to open client: {e}");
                return;
            }
        };

        let mut prev_evicted: u64 = 0;

        loop {
            match client.get_connection() {
                Ok(mut conn) => {
                    match gql_redis_store::redis_memory_metrics::poll_once(&mut conn) {
                        Ok(info) => {
                            tracing::debug!(
                                used_memory_bytes = info.used_memory,
                                maxmemory_bytes = info.maxmemory,
                                evicted_keys = info.evicted_keys,
                                "redis memory snapshot"
                            );

                            // Emit Prometheus gauges.
                            crate::metrics::record_redis_used_memory(info.used_memory);
                            crate::metrics::record_redis_maxmemory(info.maxmemory);
                            crate::metrics::record_redis_evictions(info.evicted_keys);

                            // Warn on memory pressure.
                            if info.maxmemory > 0 {
                                let pct = (info.used_memory as f64 / info.maxmemory as f64) * 100.0;
                                if pct >= 90.0 {
                                    tracing::warn!(
                                        used_memory_bytes = info.used_memory,
                                        maxmemory_bytes = info.maxmemory,
                                        usage_pct = format!("{pct:.1}"),
                                        "redis memory pressure: >= 90% of maxmemory used"
                                    );
                                }
                            }

                            // Warn on new evictions.
                            if info.evicted_keys > prev_evicted {
                                let delta = info.evicted_keys - prev_evicted;
                                tracing::warn!(
                                    new_evictions = delta,
                                    total_evicted = info.evicted_keys,
                                    "redis eviction alert: keys were evicted since last poll"
                                );
                            }
                            prev_evicted = info.evicted_keys;
                        }
                        Err(e) => tracing::warn!("redis memory poll failed: {e}"),
                    }
                }
                Err(e) => tracing::warn!("redis memory poll: connection failed: {e}"),
            }
            tokio::time::sleep(interval).await;
        }
    })
}
