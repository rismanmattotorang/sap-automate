//! In-memory TTL cache for RFC metadata — `thupalo/sap-rfc-mcp-server` pattern.
//!
//! `thupalo/sap-rfc-mcp-server` reports ~1–5 ms cached metadata reads vs
//! ~200–500 ms direct calls.  For SAP-Automate the same pattern is a
//! decorator over any [`SapClient`]: it intercepts `rfc_metadata` and
//! `bulk_rfc_metadata`, serves hits from a key-`(function, language)`
//! map, and falls through to the inner client on miss.
//!
//! **Scope (Karpathy "Simplicity First"):** in-memory only.  No
//! filesystem persistence, no compression, no LRU eviction — TTL is the
//! single eviction policy.  Easy to swap a filesystem backend in
//! behind the [`MetadataStore`] trait later if real load demands it.
//!
//! **Concurrency:** the inner map is wrapped in a `tokio::sync::RwLock`.
//! Reads are non-blocking; misses serialise per function (consistent
//! with the underlying SAP backend's pool-size knob).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::client::{
    BulkMetadata, PoolStatus, ReadTableRequest, RfcCallRequest, RfcFunctionMeta,
    RfcSearchResult, SapClient, SystemInfo, TableRow, TableStructure,
};
use crate::error::RfcResult;

/// Cache statistics surfaced to Prometheus / TUI.
#[derive(Debug, Default, Clone, Copy)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub entries: usize,
}

impl CacheStats {
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 { 0.0 } else { self.hits as f64 / total as f64 }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    meta: RfcFunctionMeta,
    cached_at: Instant,
}

/// Decorator that caches `RfcFunctionMeta` keyed by `(function, language)`
/// for a configurable TTL.
///
/// Construct with [`MetadataCache::new`] and pass the wrapped instance
/// into anything that takes `Arc<dyn SapClient>`.
pub struct MetadataCache<C: SapClient + ?Sized> {
    inner: Arc<C>,
    ttl: Duration,
    entries: RwLock<HashMap<(String, String), Entry>>,
    stats: RwLock<CacheStats>,
}

impl<C: SapClient + ?Sized> MetadataCache<C> {
    /// Wrap `inner` with a TTL cache.  TTL of 0 disables caching (every
    /// call falls through) — useful in tests.
    pub fn new(inner: Arc<C>, ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner,
            ttl,
            entries: RwLock::new(HashMap::new()),
            stats: RwLock::new(CacheStats::default()),
        })
    }

    /// Current cache stats (cheap clone of a `Copy` struct).
    pub async fn stats(&self) -> CacheStats {
        let mut s = *self.stats.read().await;
        s.entries = self.entries.read().await.len();
        s
    }

    /// Drop every entry.  Useful on system role flip (DEV→QAS) or after
    /// an SAP-side transport import that may have changed signatures.
    pub async fn invalidate_all(&self) {
        let mut entries = self.entries.write().await;
        let evicted = entries.len() as u64;
        entries.clear();
        self.stats.write().await.evictions += evicted;
    }

    async fn get_fresh(&self, key: &(String, String)) -> Option<RfcFunctionMeta> {
        let entries = self.entries.read().await;
        let e = entries.get(key)?;
        if self.ttl.is_zero() || e.cached_at.elapsed() <= self.ttl {
            Some(e.meta.clone())
        } else {
            None
        }
    }

    async fn store(&self, key: (String, String), meta: RfcFunctionMeta) {
        if self.ttl.is_zero() {
            return;
        }
        let mut entries = self.entries.write().await;
        entries.insert(key, Entry { meta, cached_at: Instant::now() });
    }
}

#[async_trait]
impl<C: SapClient + ?Sized> SapClient for MetadataCache<C> {
    async fn system_info(&self) -> RfcResult<SystemInfo> {
        self.inner.system_info().await
    }

    async fn search_rfc(&self, query: &str, limit: usize) -> RfcResult<RfcSearchResult> {
        self.inner.search_rfc(query, limit).await
    }

    async fn rfc_metadata(&self, function: &str, language: &str) -> RfcResult<RfcFunctionMeta> {
        let key = (function.to_string(), language.to_string());
        if self.ttl.is_zero() {
            self.stats.write().await.misses += 1;
            return self.inner.rfc_metadata(function, language).await;
        }
        if let Some(meta) = self.get_fresh(&key).await {
            self.stats.write().await.hits += 1;
            return Ok(meta);
        }
        // Miss — go to inner and store on success.
        self.stats.write().await.misses += 1;
        let meta = self.inner.rfc_metadata(function, language).await?;
        self.store(key, meta.clone()).await;
        Ok(meta)
    }

    async fn bulk_rfc_metadata(&self, functions: &[String], language: &str) -> RfcResult<BulkMetadata> {
        // Split into hits and misses, then fetch only the misses from
        // the inner client in one batched call.
        let mut hits: Vec<RfcFunctionMeta> = Vec::new();
        let mut to_fetch: Vec<String> = Vec::new();
        for f in functions {
            let key = (f.clone(), language.to_string());
            match self.get_fresh(&key).await {
                Some(meta) => hits.push(meta),
                None => to_fetch.push(f.clone()),
            }
        }
        {
            let mut stats = self.stats.write().await;
            stats.hits += hits.len() as u64;
            stats.misses += to_fetch.len() as u64;
        }
        if to_fetch.is_empty() {
            return Ok(BulkMetadata {
                language: language.into(),
                functions: hits,
                missing: Vec::new(),
            });
        }
        let fetched = self.inner.bulk_rfc_metadata(&to_fetch, language).await?;
        for meta in &fetched.functions {
            self.store((meta.function.clone(), language.to_string()), meta.clone()).await;
        }
        let mut out = hits;
        out.extend(fetched.functions);
        Ok(BulkMetadata {
            language: language.into(),
            functions: out,
            missing: fetched.missing,
        })
    }

    async fn call_rfc(&self, request: RfcCallRequest, read_only_mode: bool) -> RfcResult<serde_json::Value> {
        self.inner.call_rfc(request, read_only_mode).await
    }

    async fn read_table(&self, request: ReadTableRequest) -> RfcResult<Vec<TableRow>> {
        self.inner.read_table(request).await
    }

    async fn table_structure(&self, table: &str) -> RfcResult<TableStructure> {
        self.inner.table_structure(table).await
    }

    fn pool_status(&self) -> PoolStatus {
        self.inner.pool_status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::MockSapClient;

    fn mock() -> Arc<MockSapClient> {
        MockSapClient::new(4, serde_json::json!({}))
    }

    #[tokio::test]
    async fn first_read_is_miss_second_is_hit() {
        let cache = MetadataCache::new(mock(), Duration::from_secs(60));
        // Function names match the MockSapClient seed_functions() fixture.
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "EN").await.unwrap();
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "EN").await.unwrap();
        let stats = cache.stats().await;
        assert_eq!(stats.misses, 1, "first read should be a miss");
        assert_eq!(stats.hits, 1, "second read should be a hit");
        assert_eq!(stats.entries, 1);
        assert!(stats.hit_ratio() > 0.4);
    }

    #[tokio::test]
    async fn ttl_zero_disables_cache() {
        let cache = MetadataCache::new(mock(), Duration::ZERO);
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "EN").await.unwrap();
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "EN").await.unwrap();
        let stats = cache.stats().await;
        assert_eq!(stats.misses, 2, "ttl=0 forces every call to miss");
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.entries, 0, "ttl=0 never stores");
    }

    #[tokio::test]
    async fn ttl_expiry_re_fetches() {
        let cache = MetadataCache::new(mock(), Duration::from_millis(20));
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "EN").await.unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "EN").await.unwrap();
        let stats = cache.stats().await;
        assert_eq!(stats.misses, 2, "expired entry should miss");
        assert_eq!(stats.hits, 0);
    }

    #[tokio::test]
    async fn bulk_splits_hits_and_misses() {
        let cache = MetadataCache::new(mock(), Duration::from_secs(60));
        // Prime one entry.
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "EN").await.unwrap();
        let stats_before = cache.stats().await;
        assert_eq!(stats_before.misses, 1);

        let bulk = cache
            .bulk_rfc_metadata(
                &["BAPI_MATERIAL_GET_DETAIL".into(), "RFC_READ_TABLE".into()],
                "EN",
            )
            .await
            .unwrap();
        assert_eq!(bulk.functions.len(), 2);
        let stats = cache.stats().await;
        assert_eq!(stats.hits, 1, "BAPI_MATERIAL_GET_DETAIL was cached");
        assert_eq!(stats.misses, 2, "RFC_READ_TABLE was a miss (+ the primer)");
    }

    #[tokio::test]
    async fn invalidate_clears_entries_and_counts_evictions() {
        let cache = MetadataCache::new(mock(), Duration::from_secs(60));
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "EN").await.unwrap();
        let _ = cache.rfc_metadata("RFC_READ_TABLE", "EN").await.unwrap();
        assert_eq!(cache.stats().await.entries, 2);
        cache.invalidate_all().await;
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.evictions, 2);
    }

    #[tokio::test]
    async fn language_is_part_of_the_key() {
        let cache = MetadataCache::new(mock(), Duration::from_secs(60));
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "EN").await.unwrap();
        let _ = cache.rfc_metadata("BAPI_MATERIAL_GET_DETAIL", "DE").await.unwrap();
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 2, "EN and DE are separate cache entries");
        assert_eq!(stats.misses, 2);
    }
}
