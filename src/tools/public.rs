//! Public `Read` tool family — market data backed by `deribit-http`.
//!
//! All tools in this module have [`ToolClass::Read`] and require no
//! credentials. Each tool is a thin translation step over a single
//! `deribit_http::DeribitHttpClient` call (ADR-0001):
//!
//! v0.1-10 (per-instrument market data):
//!
//! - `get_ticker` — latest ticker for an instrument.
//! - `get_instrument` — instrument metadata.
//! - `list_instruments` — per-currency instrument list, filterable by
//!   kind / expiry.
//! - `get_order_book` — order book for an instrument.
//! - `get_index_price` — index price for an index name.
//!
//! v0.1-11 (summaries & meta):
//!
//! - `get_book_summary_by_currency` — book + 24h stats per instrument
//!   of a currency.
//! - `get_book_summary_by_instrument` — same for a single instrument.
//! - `get_currencies` — supported-currencies catalogue.
//! - `get_server_time` — Deribit server time.
//! - `get_status` — platform-wide status (locked currencies).
//! - `get_last_trades` — recent trades for an instrument.
//! - `get_tradingview_chart_data` — OHLCV bars over a window.
//! - `get_funding_rate_history` — funding rates over a window.
//! - `get_historical_volatility` — realised vol time series for a
//!   currency.
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

use super::schema::{parse_input as parse, schema_for};
use super::{ToolClass, ToolEntry, ToolHandlerFn, ToolRegistry};
use crate::context::AdapterContext;
use crate::error::AdapterError;

/// Register every public `Read` tool with the registry.
pub fn register(registry: &mut ToolRegistry) {
    // v0.1-10 — per-instrument market data.
    registry.insert(get_ticker_tool());
    registry.insert(get_instrument_tool());
    registry.insert(list_instruments_tool());
    registry.insert(get_order_book_tool());
    registry.insert(get_index_price_tool());
    // v0.1-11 — summaries & meta.
    registry.insert(get_book_summary_by_currency_tool());
    registry.insert(get_book_summary_by_instrument_tool());
    registry.insert(get_currencies_tool());
    registry.insert(get_server_time_tool());
    registry.insert(get_status_tool());
    registry.insert(get_last_trades_tool());
    registry.insert(get_tradingview_chart_data_tool());
    registry.insert(get_funding_rate_history_tool());
    registry.insert(get_historical_volatility_tool());
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

// ----- get_book_summary_by_currency ---------------------------------

/// `get_book_summary_by_currency` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetBookSummaryByCurrencyInput {
    /// Currency symbol, e.g. `BTC`.
    pub currency: String,
    /// Optional instrument-kind filter: `future`, `option`, `spot`,
    /// `future_combo`, `option_combo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

fn get_book_summary_by_currency_tool() -> ToolEntry {
    let schema = schema_for::<GetBookSummaryByCurrencyInput>();
    let descriptor = Tool::new(
        "get_book_summary_by_currency",
        "Best-bid/ask + 24h stats for every instrument of a currency, optionally filtered by kind.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_book_summary_by_currency(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_book_summary_by_currency(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetBookSummaryByCurrencyInput = parse(input)?;
    let result = ctx
        .http
        .get_book_summary_by_currency(&input.currency, input.kind.as_deref())
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_book_summary_by_instrument -------------------------------

/// `get_book_summary_by_instrument` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetBookSummaryByInstrumentInput {
    /// Instrument identifier.
    pub instrument_name: String,
}

fn get_book_summary_by_instrument_tool() -> ToolEntry {
    let schema = schema_for::<GetBookSummaryByInstrumentInput>();
    let descriptor = Tool::new(
        "get_book_summary_by_instrument",
        "Best-bid/ask + 24h stats for a single instrument.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_book_summary_by_instrument(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_book_summary_by_instrument(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetBookSummaryByInstrumentInput = parse(input)?;
    let result = ctx
        .http
        .get_book_summary_by_instrument(&input.instrument_name)
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_currencies ----------------------------------------------

/// `get_currencies` input — empty.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetCurrenciesInput {}

fn get_currencies_tool() -> ToolEntry {
    let schema = schema_for::<GetCurrenciesInput>();
    let descriptor = Tool::new(
        "get_currencies",
        "Catalogue of supported currencies and their metadata.",
        schema,
    );
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_get_currencies(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_currencies(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let _: GetCurrenciesInput = parse(input)?;
    let result = ctx.http.get_currencies().await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_server_time ---------------------------------------------

/// `get_server_time` input — empty.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetServerTimeInput {}

fn get_server_time_tool() -> ToolEntry {
    let schema = schema_for::<GetServerTimeInput>();
    let descriptor = Tool::new(
        "get_server_time",
        "Current Deribit server time as a Unix epoch in milliseconds.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_server_time(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_server_time(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let _: GetServerTimeInput = parse(input)?;
    let result = ctx.http.get_server_time().await?;
    Ok(serde_json::to_value(result)?)
}

// ----- get_status --------------------------------------------------

/// `get_status` input — empty.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetStatusInput {}

fn get_status_tool() -> ToolEntry {
    let schema = schema_for::<GetStatusInput>();
    let descriptor = Tool::new(
        "get_status",
        "Platform-wide status: locked currencies, partial outages.",
        schema,
    );
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_get_status(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_status(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let _: GetStatusInput = parse(input)?;
    let result = ctx.http.get_status().await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_last_trades ---------------------------------------------

/// `get_last_trades` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetLastTradesInput {
    /// Instrument identifier.
    pub instrument_name: String,
    /// How many trades to return (upstream caps at ~1 000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// When `true`, include trades from before the last archive boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_old: Option<bool>,
}

fn get_last_trades_tool() -> ToolEntry {
    let schema = schema_for::<GetLastTradesInput>();
    let descriptor = Tool::new(
        "get_last_trades",
        "Recent trades for an instrument, newest first.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_last_trades(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_last_trades(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: GetLastTradesInput = parse(input)?;
    let result = ctx
        .http
        .get_last_trades(&input.instrument_name, input.count, input.include_old)
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_tradingview_chart_data ----------------------------------

/// `get_tradingview_chart_data` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetTradingViewChartDataInput {
    /// Instrument identifier.
    pub instrument_name: String,
    /// Start of the window, Unix epoch milliseconds.
    pub start_timestamp: u64,
    /// End of the window, Unix epoch milliseconds.
    pub end_timestamp: u64,
    /// Bar resolution: `1`, `3`, `5`, `10`, `15`, `30`, `60`, `120`,
    /// `180`, `360`, `720`, `1D`.
    pub resolution: String,
}

fn get_tradingview_chart_data_tool() -> ToolEntry {
    let schema = schema_for::<GetTradingViewChartDataInput>();
    let descriptor = Tool::new(
        "get_tradingview_chart_data",
        "OHLCV bars for an instrument over a time window at a chosen resolution.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_tradingview_chart_data(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_tradingview_chart_data(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetTradingViewChartDataInput = parse(input)?;
    let result = ctx
        .http
        .get_tradingview_chart_data(
            &input.instrument_name,
            input.start_timestamp,
            input.end_timestamp,
            &input.resolution,
        )
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_funding_rate_history ------------------------------------

/// `get_funding_rate_history` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetFundingRateHistoryInput {
    /// Perpetual instrument identifier (e.g. `BTC-PERPETUAL`).
    pub instrument_name: String,
    /// Start of the window, Unix epoch milliseconds.
    pub start_timestamp: u64,
    /// End of the window, Unix epoch milliseconds.
    pub end_timestamp: u64,
}

fn get_funding_rate_history_tool() -> ToolEntry {
    let schema = schema_for::<GetFundingRateHistoryInput>();
    let descriptor = Tool::new(
        "get_funding_rate_history",
        "Historical funding rates for a perpetual instrument over a window.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_funding_rate_history(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_funding_rate_history(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetFundingRateHistoryInput = parse(input)?;
    let result = ctx
        .http
        .get_funding_rate_history(
            &input.instrument_name,
            input.start_timestamp,
            input.end_timestamp,
        )
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_historical_volatility -----------------------------------

/// `get_historical_volatility` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetHistoricalVolatilityInput {
    /// Currency symbol (`BTC`, `ETH`).
    pub currency: String,
}

fn get_historical_volatility_tool() -> ToolEntry {
    let schema = schema_for::<GetHistoricalVolatilityInput>();
    let descriptor = Tool::new(
        "get_historical_volatility",
        "Historical realised volatility (`[timestamp_ms, value]`) for a currency.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_historical_volatility(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Read,
        handler,
    }
}

async fn handle_get_historical_volatility(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetHistoricalVolatilityInput = parse(input)?;
    let result = ctx.http.get_historical_volatility(&input.currency).await?;
    Ok(serde_json::to_value(&result)?)
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

    /// Every tool in this module registers with `ToolClass::Read`.
    #[test]
    fn all_public_tools_are_read_class() {
        for entry in [
            get_ticker_tool(),
            get_instrument_tool(),
            list_instruments_tool(),
            get_order_book_tool(),
            get_index_price_tool(),
            get_book_summary_by_currency_tool(),
            get_book_summary_by_instrument_tool(),
            get_currencies_tool(),
            get_server_time_tool(),
            get_status_tool(),
            get_last_trades_tool(),
            get_tradingview_chart_data_tool(),
            get_funding_rate_history_tool(),
            get_historical_volatility_tool(),
        ] {
            assert_eq!(entry.class, ToolClass::Read);
        }
    }

    #[test]
    fn register_populates_full_v01_set() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        let listed = registry.list();
        let names: Vec<&str> = listed.iter().map(|t| t.name.as_ref()).collect();
        for expected in [
            "get_book_summary_by_currency",
            "get_book_summary_by_instrument",
            "get_currencies",
            "get_funding_rate_history",
            "get_historical_volatility",
            "get_index_price",
            "get_instrument",
            "get_last_trades",
            "get_order_book",
            "get_server_time",
            "get_status",
            "get_ticker",
            "get_tradingview_chart_data",
            "list_instruments",
        ] {
            assert!(
                names.contains(&expected),
                "missing tool {expected}; got {names:?}"
            );
        }
        // v0.1 ships exactly these 14 Read-class tools (5 from
        // v0.1-10, 9 from v0.1-11).
        assert_eq!(registry.len(), 14);
    }

    #[test]
    fn empty_input_tools_accept_empty_payload() {
        for tool in ["get_currencies", "get_server_time", "get_status"] {
            let mut registry = ToolRegistry::new();
            register(&mut registry);
            assert!(registry.contains(tool), "{tool} registered");
        }
        // Schemas for empty-input tools deserialize a `{}` cleanly.
        let _: GetCurrenciesInput = serde_json::from_value(serde_json::json!({})).unwrap();
        let _: GetServerTimeInput = serde_json::from_value(serde_json::json!({})).unwrap();
        let _: GetStatusInput = serde_json::from_value(serde_json::json!({})).unwrap();
    }

    #[test]
    fn last_trades_input_accepts_required_only() {
        let v = serde_json::json!({"instrument_name": "BTC-PERPETUAL"});
        let parsed: GetLastTradesInput = serde_json::from_value(v).expect("parse");
        assert!(parsed.count.is_none());
        assert!(parsed.include_old.is_none());
    }

    #[test]
    fn chart_data_input_requires_window_and_resolution() {
        // missing resolution
        let v = serde_json::json!({
            "instrument_name": "BTC-PERPETUAL",
            "start_timestamp": 0u64,
            "end_timestamp": 1u64,
        });
        let err = parse::<GetTradingViewChartDataInput>(v).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { .. }));
    }
}
