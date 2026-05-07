//! Public `Read` tool family — market data backed by `deribit-http`.
//!
//! All tools in this module have [`ToolClass::Read`] and require no
//! credentials. Each tool is a thin translation step over a single
//! `deribit_http::DeribitHttpClient` call (ADR-0001):
//!
//! - `get_ticker` — latest ticker for an instrument.
//! - `get_instrument` — instrument metadata.
//! - `list_instruments` — per-currency instrument list, filterable by
//!   kind / expiry.
//! - `get_order_book` — order book for an instrument.
//! - `get_index_price` — index price for an index name.
//!
//! Inputs are typed structs deriving `JsonSchema` so the LLM client
//! sees a precise schema. Outputs are `serde_json::Value` carrying
//! the upstream JSON payload verbatim; v0.2+ may tighten to typed
//! outputs once the upstream response shapes stabilise.
//!
//! [`ToolClass::Read`]: super::ToolClass::Read

use std::sync::Arc;

use rmcp::model::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ToolClass, ToolEntry, ToolHandlerFn, ToolRegistry};
use crate::context::AdapterContext;
use crate::error::AdapterError;

/// Register every public `Read` tool with the registry.
pub fn register(registry: &mut ToolRegistry) {
    registry.insert(get_ticker_tool());
    registry.insert(get_instrument_tool());
    registry.insert(list_instruments_tool());
    registry.insert(get_order_book_tool());
    registry.insert(get_index_price_tool());
}

// ----- get_ticker --------------------------------------------------

/// `get_ticker` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetTickerInput {
    /// Instrument identifier, e.g. `BTC-PERPETUAL` or
    /// `BTC-31MAY24-50000-C`.
    pub instrument_name: String,
}

fn get_ticker_tool() -> ToolEntry {
    let schema = schema_for::<GetTickerInput>();
    let descriptor = Tool::new(
        "get_ticker",
        "Latest ticker (best bid / ask, mark price, last price) for an instrument.",
        schema,
    );
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_get_ticker(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_ticker(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: GetTickerInput = parse(input)?;
    let result = ctx.http.get_ticker(&input.instrument_name).await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_instrument ----------------------------------------------

/// `get_instrument` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetInstrumentInput {
    /// Instrument identifier, e.g. `BTC-PERPETUAL`.
    pub instrument_name: String,
}

fn get_instrument_tool() -> ToolEntry {
    let schema = schema_for::<GetInstrumentInput>();
    let descriptor = Tool::new(
        "get_instrument",
        "Static metadata for an instrument: type, tick size, contract size, expiry.",
        schema,
    );
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_get_instrument(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_instrument(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: GetInstrumentInput = parse(input)?;
    let result = ctx.http.get_instrument(&input.instrument_name).await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- list_instruments --------------------------------------------

/// `list_instruments` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ListInstrumentsInput {
    /// Currency symbol to list instruments for, e.g. `BTC`.
    pub currency: String,
    /// Optional kind filter: `future`, `option`, `spot`,
    /// `future_combo`, `option_combo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// When `true`, include expired instruments. Defaults to `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired: Option<bool>,
}

fn list_instruments_tool() -> ToolEntry {
    let schema = schema_for::<ListInstrumentsInput>();
    let descriptor = Tool::new(
        "list_instruments",
        "List instruments for a currency, filterable by kind and expiry status.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_list_instruments(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_list_instruments(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: ListInstrumentsInput = parse(input)?;
    let result = ctx
        .http
        .get_instruments(&input.currency, input.kind.as_deref(), input.expired)
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_order_book ----------------------------------------------

/// `get_order_book` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetOrderBookInput {
    /// Instrument identifier.
    pub instrument_name: String,
    /// Order book depth per side (1, 5, 10, 20, 50, 100, 1000,
    /// 10000). Defaults to upstream's default (5) when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
}

fn get_order_book_tool() -> ToolEntry {
    let schema = schema_for::<GetOrderBookInput>();
    let descriptor = Tool::new(
        "get_order_book",
        "Order book snapshot for an instrument with optional depth.",
        schema,
    );
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_get_order_book(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_order_book(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: GetOrderBookInput = parse(input)?;
    let result = ctx
        .http
        .get_order_book(&input.instrument_name, input.depth)
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_index_price ---------------------------------------------

/// `get_index_price` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetIndexPriceInput {
    /// Index name, e.g. `btc_usd`, `eth_usd`.
    pub index_name: String,
}

fn get_index_price_tool() -> ToolEntry {
    let schema = schema_for::<GetIndexPriceInput>();
    let descriptor = Tool::new(
        "get_index_price",
        "Current Deribit index price for an index name.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_index_price(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_index_price(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: GetIndexPriceInput = parse(input)?;
    let result = ctx.http.get_index_price(&input.index_name).await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- helpers ------------------------------------------------------

/// Parse the JSON arguments into the typed input struct.
///
/// Surfaces a structured [`AdapterError::Validation`] with the
/// upstream serde error message so the LLM sees what is wrong rather
/// than an opaque parse failure.
fn parse<T: for<'de> serde::Deserialize<'de>>(input: Value) -> Result<T, AdapterError> {
    serde_json::from_value::<T>(input).map_err(|err| AdapterError::Validation {
        field: "arguments".to_string(),
        message: err.to_string(),
    })
}

/// Build an `Arc<JsonObject>` schema for a `JsonSchema` type.
fn schema_for<T: JsonSchema>() -> Arc<serde_json::Map<String, Value>> {
    let schema = schemars::schema_for!(T);
    let json = serde_json::to_value(schema).expect("schema must be a JSON object");
    let map = json
        .as_object()
        .expect("schemars schema_for! always produces an object")
        .clone();
    Arc::new(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_value() -> Value {
        Value::Object(Default::default())
    }

    #[test]
    fn schema_for_get_ticker_lists_required_field() {
        let schema = schema_for::<GetTickerInput>();
        let required = schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required array");
        assert!(
            required.iter().any(|v| v == "instrument_name"),
            "schema must require instrument_name"
        );
    }

    #[test]
    fn list_instruments_input_accepts_optional_kind() {
        let v = serde_json::json!({"currency": "BTC"});
        let parsed: ListInstrumentsInput = serde_json::from_value(v).expect("parse");
        assert_eq!(parsed.currency, "BTC");
        assert!(parsed.kind.is_none());
        assert!(parsed.expired.is_none());
    }

    #[test]
    fn get_order_book_input_accepts_omitted_depth() {
        let v = serde_json::json!({"instrument_name": "BTC-PERPETUAL"});
        let parsed: GetOrderBookInput = serde_json::from_value(v).expect("parse");
        assert!(parsed.depth.is_none());
    }

    #[test]
    fn parse_returns_validation_error_on_bad_input() {
        let err = parse::<GetTickerInput>(empty_value()).unwrap_err();
        match err {
            AdapterError::Validation { field, .. } => assert_eq!(field, "arguments"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Compile-time assertion that every public tool registers with
    /// `ToolClass::Read`.
    #[test]
    fn all_public_tools_are_read_class() {
        for entry in [
            get_ticker_tool(),
            get_instrument_tool(),
            list_instruments_tool(),
            get_order_book_tool(),
            get_index_price_tool(),
        ] {
            assert_eq!(entry.class, ToolClass::Read);
        }
    }

    #[test]
    fn register_populates_five_tools() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        assert_eq!(registry.len(), 5);
        let listed = registry.list();
        let names: Vec<&str> = listed.iter().map(|t| t.name.as_ref()).collect();
        for expected in [
            "get_index_price",
            "get_instrument",
            "get_order_book",
            "get_ticker",
            "list_instruments",
        ] {
            assert!(
                names.contains(&expected),
                "missing tool {expected}; got {names:?}"
            );
        }
    }
}
