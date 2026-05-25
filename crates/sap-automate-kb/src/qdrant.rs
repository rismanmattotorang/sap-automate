//! Qdrant REST client.
//!
//! Wraps the Qdrant HTTP API (no gRPC, no protobuf) so the build stays
//! lightweight.  Implements `KnowledgeStore::upsert` and `::search` against
//! one collection per `Domain` (paper §VI-G).
//!
//! The collection layout mirrors paper §VI-G:
//!   - vectors.size  = embedding dim (constructor arg)
//!   - vectors.distance = "Cosine"
//!   - payload schema captures the chunk fields needed for retrieval
//!     reranking without a Postgres round-trip.

use crate::schema::{Chunk, Document, DocumentId, Domain};
use crate::store::{KnowledgeStore, SearchHit, SearchQuery, StoreError, UpsertBatch};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// HTTP client wrapped around a single Qdrant endpoint.
pub struct QdrantStore {
    http: reqwest::Client,
    base_url: String,
    embedding_dim: usize,
    /// Cached document store (Phase 1A: keep parents in Postgres in P3+;
    /// for now we colocate them in payload so the store is self-contained).
    documents: Arc<tokio::sync::RwLock<HashMap<DocumentId, Document>>>,
}

impl QdrantStore {
    pub fn new(base_url: impl Into<String>, embedding_dim: usize) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            embedding_dim,
            documents: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Ensure all four domain collections exist with the expected shape.
    pub async fn ensure_collections(&self) -> Result<(), StoreError> {
        for domain in [Domain::SapHelp, Domain::Abap, Domain::Bpmn, Domain::Leanix] {
            self.ensure_collection(domain).await?;
        }
        Ok(())
    }

    async fn ensure_collection(&self, domain: Domain) -> Result<(), StoreError> {
        let name = domain.collection();
        let url = format!("{}/collections/{}", self.base_url, name);
        let resp = self.http.get(&url).send().await.map_err(req_err)?;
        if resp.status().is_success() {
            debug!(collection = name, "collection already exists");
            return Ok(());
        }
        // Create.
        let body = json!({
            "vectors": {
                "size": self.embedding_dim,
                "distance": "Cosine",
            },
        });
        let resp = self.http.put(&url).json(&body).send().await.map_err(req_err)?;
        if !resp.status().is_success() {
            let s = resp.text().await.unwrap_or_default();
            return Err(StoreError::Backend(format!("create {name}: {s}")));
        }
        info!(collection = name, dim = self.embedding_dim, "qdrant collection created");
        Ok(())
    }

    fn upsert_url(&self, domain: Domain) -> String {
        format!("{}/collections/{}/points?wait=true", self.base_url, domain.collection())
    }

    fn search_url(&self, domain: Domain) -> String {
        format!("{}/collections/{}/points/search", self.base_url, domain.collection())
    }
}

fn req_err(e: reqwest::Error) -> StoreError {
    StoreError::Backend(format!("qdrant http: {e}"))
}

#[derive(Deserialize)]
struct QdrantSearchResponse {
    result: Vec<QdrantHit>,
}

#[derive(Deserialize)]
struct QdrantHit {
    #[allow(dead_code)]
    id: Value,
    score: f32,
    payload: Value,
}

#[async_trait]
impl KnowledgeStore for QdrantStore {
    async fn upsert(&self, batch: UpsertBatch) -> Result<(), StoreError> {
        // Stash documents.
        {
            let mut docs = self.documents.write().await;
            for d in &batch.documents { docs.insert(d.id.clone(), d.clone()); }
        }

        // Group chunks by domain so each goes to its own collection.
        let mut by_domain: HashMap<Domain, Vec<&Chunk>> = HashMap::new();
        for c in &batch.chunks { by_domain.entry(c.domain).or_default().push(c); }

        for (domain, chunks) in by_domain {
            let mut points: Vec<Value> = Vec::with_capacity(chunks.len());
            for c in &chunks {
                let embedding = c.embedding.as_ref().ok_or_else(|| {
                    StoreError::Backend(format!("chunk {} missing embedding", c.id))
                })?;
                if embedding.len() != self.embedding_dim {
                    return Err(StoreError::Backend(format!(
                        "chunk {} embedding dim {} != configured {}",
                        c.id, embedding.len(), self.embedding_dim,
                    )));
                }
                points.push(json!({
                    "id": uuid_for(&c.id),
                    "vector": embedding,
                    "payload": {
                        "chunk_id": c.id,
                        "document_id": c.document_id,
                        "ordinal": c.ordinal,
                        "text": c.text,
                        "title": c.title,
                        "uri": c.uri,
                        "breadcrumbs": c.breadcrumbs,
                    }
                }));
            }
            let body = json!({ "points": points });
            let resp = self.http.put(self.upsert_url(domain)).json(&body).send().await.map_err(req_err)?;
            if !resp.status().is_success() {
                let s = resp.text().await.unwrap_or_default();
                return Err(StoreError::Backend(format!("upsert {}: {}", domain.collection(), s)));
            }
            debug!(domain = ?domain, count = chunks.len(), "upserted to qdrant");
        }
        Ok(())
    }

    async fn get_document(&self, id: &DocumentId) -> Result<Option<Document>, StoreError> {
        Ok(self.documents.read().await.get(id).cloned())
    }

    async fn search(&self, query: SearchQuery) -> Result<Vec<SearchHit>, StoreError> {
        let embedding = query.embedding.as_ref().ok_or_else(|| {
            StoreError::Backend("qdrant search requires an embedding".into())
        })?;
        let targets: Vec<Domain> = if query.domains.is_empty() {
            vec![Domain::SapHelp, Domain::Abap, Domain::Bpmn, Domain::Leanix]
        } else {
            query.domains.clone()
        };

        let mut all_hits: Vec<SearchHit> = Vec::new();
        for domain in targets {
            let body = json!({
                "vector": embedding,
                "limit": query.top_k,
                "with_payload": true,
            });
            let resp = self.http.post(self.search_url(domain)).json(&body).send().await.map_err(req_err)?;
            if !resp.status().is_success() {
                let s = resp.text().await.unwrap_or_default();
                warn!(domain = ?domain, "qdrant search failed: {s}");
                continue;
            }
            let parsed: QdrantSearchResponse = resp.json().await.map_err(req_err)?;
            for h in parsed.result {
                let chunk = chunk_from_payload(domain, &h.payload).ok_or_else(|| {
                    StoreError::Backend("malformed qdrant payload".into())
                })?;
                all_hits.push(SearchHit { chunk, score: h.score });
            }
        }
        all_hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all_hits.truncate(query.top_k);
        Ok(all_hits)
    }

    async fn chunk_count(&self) -> Result<usize, StoreError> {
        // Qdrant exposes per-collection counts; sum across the four domains.
        let mut total = 0usize;
        for domain in [Domain::SapHelp, Domain::Abap, Domain::Bpmn, Domain::Leanix] {
            let url = format!("{}/collections/{}", self.base_url, domain.collection());
            let resp = self.http.get(&url).send().await.map_err(req_err)?;
            if !resp.status().is_success() { continue; }
            let v: Value = resp.json().await.map_err(req_err)?;
            if let Some(n) = v.pointer("/result/points_count").and_then(|n| n.as_u64()) {
                total += n as usize;
            }
        }
        Ok(total)
    }
}

fn chunk_from_payload(domain: Domain, payload: &Value) -> Option<Chunk> {
    Some(Chunk {
        id: payload.get("chunk_id")?.as_str()?.to_string(),
        document_id: payload.get("document_id")?.as_str()?.to_string(),
        domain,
        ordinal: payload.get("ordinal").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        text: payload.get("text")?.as_str()?.to_string(),
        embedding: None,
        breadcrumbs: payload
            .get("breadcrumbs")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        title: payload.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        uri: payload.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string(),
    })
}

/// Qdrant requires UUID or unsigned integer point IDs.  Deterministically
/// derive a UUID v5-ish identifier from the chunk id via SHA-256 truncation.
fn uuid_for(chunk_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(chunk_id.as_bytes());
    let bytes = h.finalize();
    // Format as RFC 4122 UUID with version 5 marker.
    let mut b = [0u8; 16];
    b.copy_from_slice(&bytes[..16]);
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
    )
}
