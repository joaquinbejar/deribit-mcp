//! Shared helpers for tool input parsing and JSON-schema construction.
//!
//! Centralised so the per-family tool modules
//! (`tools::public`, `tools::account`, `tools::trading`) can't drift
//! on how they map a deserialise error onto an
//! [`AdapterError::Validation`] payload, or how they hand `schemars`
//! output to the rmcp `Tool` constructor.

use std::sync::Arc;

use schemars::JsonSchema;
use serde_json::Value;

use crate::error::AdapterError;

/// Parse the JSON arguments into the typed input struct, mapping a
/// deserialise error onto a structured
/// [`AdapterError::Validation`] with `field = "arguments"` and the
/// upstream serde message verbatim. The LLM client sees what's
/// wrong rather than an opaque parse failure.
///
/// # Errors
///
/// Returns [`AdapterError::Validation`] for any input that does not
/// match `T`'s schema.
pub fn parse_input<T: for<'de> serde::Deserialize<'de>>(input: Value) -> Result<T, AdapterError> {
    serde_json::from_value::<T>(input).map_err(|err| AdapterError::Validation {
        field: "arguments".to_string(),
        message: err.to_string(),
    })
}

/// Build an `Arc<JsonObject>` schema for a `JsonSchema` type.
///
/// Wraps `schemars::schema_for!(T)` and converts the resulting
/// `Schema` into the shape `rmcp::model::Tool::new` expects
/// (`Arc<serde_json::Map<String, Value>>`).
#[must_use]
pub fn schema_for<T: JsonSchema>() -> Arc<serde_json::Map<String, Value>> {
    let schema = schemars::schema_for!(T);
    let json = serde_json::to_value(schema).expect("schema must be a JSON object");
    let map = json
        .as_object()
        .expect("schemars schema_for! always produces an object")
        .clone();
    Arc::new(map)
}
