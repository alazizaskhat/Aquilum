pub mod api;
pub mod auth;
pub mod config;
pub mod embedded;
#[cfg(feature = "standalone")]
pub mod upstream;

use serde_json::Value;
use std::{future::Future, pin::Pin};

pub type NoteResult = Result<Value, api::ApiError>;
pub type NoteFuture<'a> = Pin<Box<dyn Future<Output = NoteResult> + Send + 'a>>;

/// The same API can call Aquilum's core directly or use a standalone MCP client.
pub trait NoteBackend: Send + Sync {
    fn call(&self, tool: &'static str, arguments: Value) -> NoteFuture<'_>;
}
