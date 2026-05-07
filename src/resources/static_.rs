//! Static (refresh-on-read) resources backed by `deribit-http`.
//!
//! Each function maps one [`ResourceUri`] variant to a single upstream
//! HTTP call and returns the JSON payload verbatim. The MIME type
//! advertised in `resources/list` is `application/json`.
//!
//! [`ResourceUri`]: super::ResourceUri

use serde_json::Value;

use crate::context::AdapterContext;
use crate::error::AdapterError;

/// Read the `deribit://currencies` resource — the supported-currencies
/// catalogue served by `public/get_currencies`.
///
/// # Errors
///
/// Returns whatever [`AdapterError`] the upstream HTTP call surfaces
/// (network, rate-limit, API).
pub async fn read_currencies(ctx: &AdapterContext) -> Result<Value, AdapterError> {
    let result = ctx.http.get_currencies().await?;
    Ok(serde_json::to_value(&result)?)
}

/// Read the `deribit://instruments/{currency}` resource — every
/// instrument for the given currency. Mirrors
/// `public/get_instruments(currency, None, None)`: kind unfiltered,
/// expired excluded.
///
/// # Errors
///
/// Returns whatever [`AdapterError`] the upstream HTTP call surfaces.
pub async fn read_instruments(ctx: &AdapterContext, currency: &str) -> Result<Value, AdapterError> {
    let result = ctx.http.get_instruments(currency, None, None).await?;
    Ok(serde_json::to_value(&result)?)
}
