//! SAP connectors (paper §IV bottom band, Phase 2).
//!
//! Phase 1 ships only the trait surface; concrete ADT / Signavio / LeanIX
//! HTTP clients land in Phase 2.

use std::future::Future;
use std::pin::Pin;

pub trait AbapConnector: Send + Sync {
    fn read_object<'a>(
        &'a self,
        package: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ConnectorError>> + Send + 'a>>;
}

pub trait BpmnConnector: Send + Sync {
    fn read_xml<'a>(
        &'a self,
        process_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ConnectorError>> + Send + 'a>>;
}

pub trait LeanixConnector: Send + Sync {
    fn read_fact_sheet<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, ConnectorError>> + Send + 'a>>;
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("auth error: {0}")]
    Auth(String),
}

