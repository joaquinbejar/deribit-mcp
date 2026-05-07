//! Authenticated `Account` tool family.
//!
//! All tools in this module have [`ToolClass::Account`] and require
//! credentials configured via `DERIBIT_CLIENT_ID` /
//! `DERIBIT_CLIENT_SECRET` (ADR-0004). The first call drives the
//! upstream `AuthManager`'s OAuth client-credentials flow lazily and
//! caches the token (v0.2-01).
//!
//! v0.2-02 ships:
//!
//! - `get_account_summary` — balance / equity / margin for a currency.
//! - `get_positions` — open positions, optionally filtered by currency
//!   / kind / subaccount.
//! - `get_subaccounts` — subaccount list with optional portfolio.
//!
//! v0.2-03 adds historical-activity tools:
//!
//! - `get_transaction_log` — account transaction log for a window.
//! - `get_deposits` — recent deposits for a currency.
//! - `get_withdrawals` — recent withdrawals for a currency.
//!
//! v0.2-04 adds order + user-trades history:
//!
//! - `get_open_orders_by_currency` — open orders for a currency,
//!   optionally filtered by kind / type.
//! - `get_open_orders_by_instrument` — open orders for one
//!   instrument.
//! - `get_user_trades_by_currency` — user trades over an id /
//!   timestamp window.
//! - `get_user_trades_by_instrument` — user trades for an
//!   instrument over a sequence-number window.
//!
//! [`ToolClass::Account`]: super::ToolClass::Account

use std::sync::Arc;

use rmcp::model::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::schema::{parse_input as parse, schema_for};
use super::{ToolClass, ToolEntry, ToolHandlerFn, ToolRegistry};
use crate::context::AdapterContext;
use crate::error::AdapterError;

/// Register every `Account` tool with the registry.
pub fn register(registry: &mut ToolRegistry) {
    registry.insert(get_account_summary_tool());
    registry.insert(get_positions_tool());
    registry.insert(get_subaccounts_tool());
    // v0.2-03 — historical activity.
    registry.insert(get_transaction_log_tool());
    registry.insert(get_deposits_tool());
    registry.insert(get_withdrawals_tool());
    // v0.2-04 — orders + user-trades history.
    registry.insert(get_open_orders_by_currency_tool());
    registry.insert(get_open_orders_by_instrument_tool());
    registry.insert(get_user_trades_by_currency_tool());
    registry.insert(get_user_trades_by_instrument_tool());
}

// ----- get_account_summary ------------------------------------------

/// `get_account_summary` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetAccountSummaryInput {
    /// Currency to summarise (`BTC`, `ETH`, `USDC`, …).
    pub currency: String,
    /// Include the per-currency `summaries[]` (id, email, account type, …).
    /// Defaults to `false` upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extended: Option<bool>,
}

fn get_account_summary_tool() -> ToolEntry {
    let schema = schema_for::<GetAccountSummaryInput>();
    let descriptor = Tool::new(
        "get_account_summary",
        "Account balance / equity / margin for a single currency.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_account_summary(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_account_summary(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetAccountSummaryInput = parse(input)?;
    let result = ctx
        .http
        .get_account_summary(&input.currency, input.extended)
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_positions ------------------------------------------------

/// `get_positions` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetPositionsInput {
    /// Optional currency filter (`BTC`, `ETH`, …). When omitted the
    /// upstream returns positions across every currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Optional instrument-kind filter: `future`, `option`, `spot`,
    /// `future_combo`, `option_combo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Optional subaccount id to scope the query to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subaccount_id: Option<i32>,
}

fn get_positions_tool() -> ToolEntry {
    let schema = schema_for::<GetPositionsInput>();
    let descriptor = Tool::new(
        "get_positions",
        "Open positions, optionally filtered by currency / kind / subaccount.",
        schema,
    );
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_get_positions(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_positions(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: GetPositionsInput = parse(input)?;
    let result = ctx
        .http
        .get_positions(
            input.currency.as_deref(),
            input.kind.as_deref(),
            input.subaccount_id,
        )
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_subaccounts ----------------------------------------------

/// `get_subaccounts` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetSubaccountsInput {
    /// When `true`, include each subaccount's portfolio in the
    /// response. Defaults to `false` upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_portfolio: Option<bool>,
}

fn get_subaccounts_tool() -> ToolEntry {
    let schema = schema_for::<GetSubaccountsInput>();
    let descriptor = Tool::new(
        "get_subaccounts",
        "List subaccounts, optionally including portfolio per subaccount.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_subaccounts(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_subaccounts(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: GetSubaccountsInput = parse(input)?;
    let result = ctx.http.get_subaccounts(input.with_portfolio).await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_transaction_log ------------------------------------------

/// `get_transaction_log` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetTransactionLogInput {
    /// Currency to scope the log to (`BTC`, `ETH`, …).
    pub currency: String,
    /// Window start, Unix epoch milliseconds.
    pub start_timestamp: u64,
    /// Window end, Unix epoch milliseconds.
    pub end_timestamp: u64,
    /// Optional substring search across the log entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Maximum entries to return (upstream caps the page size).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// Optional subaccount id to scope the log to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subaccount_id: Option<u64>,
    /// Continuation token from a previous page (returned in
    /// upstream `continuation` field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<u64>,
}

fn get_transaction_log_tool() -> ToolEntry {
    let schema = schema_for::<GetTransactionLogInput>();
    let descriptor = Tool::new(
        "get_transaction_log",
        "Account transaction log for a currency over a window, with optional pagination.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_transaction_log(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_transaction_log(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetTransactionLogInput = parse(input)?;
    let request = deribit_http::model::transaction::TransactionLogRequest {
        currency: input.currency,
        start_timestamp: input.start_timestamp,
        end_timestamp: input.end_timestamp,
        query: input.query,
        count: input.count,
        subaccount_id: input.subaccount_id,
        continuation: input.continuation,
    };
    let result = ctx.http.get_transaction_log(request).await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_deposits / get_withdrawals -------------------------------

/// Pagination + currency input shared by `get_deposits` and
/// `get_withdrawals` (the upstream signatures are identical).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct PaginatedCurrencyInput {
    /// Currency to scope the query to (`BTC`, `ETH`, …).
    pub currency: String,
    /// Page size; defaults to upstream's default (10) when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// Page offset; defaults to 0 when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

fn get_deposits_tool() -> ToolEntry {
    let schema = schema_for::<PaginatedCurrencyInput>();
    let descriptor = Tool::new(
        "get_deposits",
        "Recent deposits for a currency, paginated.",
        schema,
    );
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_get_deposits(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_deposits(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: PaginatedCurrencyInput = parse(input)?;
    let result = ctx
        .http
        .get_deposits(&input.currency, input.count, input.offset)
        .await?;
    Ok(serde_json::to_value(&result)?)
}

fn get_withdrawals_tool() -> ToolEntry {
    let schema = schema_for::<PaginatedCurrencyInput>();
    let descriptor = Tool::new(
        "get_withdrawals",
        "Recent withdrawals for a currency, paginated.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_withdrawals(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_withdrawals(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: PaginatedCurrencyInput = parse(input)?;
    let result = ctx
        .http
        .get_withdrawals(&input.currency, input.count, input.offset)
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_open_orders_by_currency / by_instrument ------------------

/// `get_open_orders_by_currency` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetOpenOrdersByCurrencyInput {
    /// Currency (`BTC`, `ETH`, …).
    pub currency: String,
    /// Optional instrument-kind filter: `future`, `option`, `spot`,
    /// `future_combo`, `option_combo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Optional order-type filter (`limit`, `stop_limit`, `take_limit`,
    /// `market`, `stop_market`, `take_market`, `market_limit`,
    /// `trailing_stop`, `all`).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub order_type: Option<String>,
}

fn get_open_orders_by_currency_tool() -> ToolEntry {
    let schema = schema_for::<GetOpenOrdersByCurrencyInput>();
    let descriptor = Tool::new(
        "get_open_orders_by_currency",
        "Open orders for a currency, optionally filtered by kind and type.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_open_orders_by_currency(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_open_orders_by_currency(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetOpenOrdersByCurrencyInput = parse(input)?;
    let result = ctx
        .http
        .get_open_orders_by_currency(
            &input.currency,
            input.kind.as_deref(),
            input.order_type.as_deref(),
        )
        .await?;
    Ok(serde_json::to_value(&result)?)
}

/// `get_open_orders_by_instrument` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetOpenOrdersByInstrumentInput {
    /// Instrument identifier (`BTC-PERPETUAL`, …).
    pub instrument_name: String,
    /// Optional order-type filter; same vocabulary as
    /// [`GetOpenOrdersByCurrencyInput::order_type`].
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub order_type: Option<String>,
}

fn get_open_orders_by_instrument_tool() -> ToolEntry {
    let schema = schema_for::<GetOpenOrdersByInstrumentInput>();
    let descriptor = Tool::new(
        "get_open_orders_by_instrument",
        "Open orders for a single instrument, optionally filtered by type.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_open_orders_by_instrument(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_open_orders_by_instrument(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetOpenOrdersByInstrumentInput = parse(input)?;
    let result = ctx
        .http
        .get_open_orders_by_instrument(&input.instrument_name, input.order_type.as_deref())
        .await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_user_trades_by_currency ----------------------------------

/// `get_user_trades_by_currency` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetUserTradesByCurrencyInput {
    /// Currency (`BTC`, `ETH`, …). Forwarded to the upstream as a
    /// closed-set `Currency` enum.
    pub currency: String,
    /// Optional instrument-kind filter: `future`, `option`, `spot`,
    /// `future_combo`, `option_combo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// First trade id to return (string per upstream spec).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_id: Option<String>,
    /// Last trade id to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_id: Option<String>,
    /// Page size (1..=1000; upstream default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// Earliest timestamp to filter on (epoch ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_timestamp: Option<u64>,
    /// Latest timestamp to filter on (epoch ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_timestamp: Option<u64>,
    /// Sort direction: `asc` or `desc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sorting: Option<String>,
    /// When `true`, include archived trades.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub historical: Option<bool>,
    /// Optional subaccount id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subaccount_id: Option<u32>,
}

fn get_user_trades_by_currency_tool() -> ToolEntry {
    let schema = schema_for::<GetUserTradesByCurrencyInput>();
    let descriptor = Tool::new(
        "get_user_trades_by_currency",
        "User trades for a currency over an id / timestamp window with sort + historical opt-in.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_user_trades_by_currency(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_user_trades_by_currency(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetUserTradesByCurrencyInput = parse(input)?;
    use deribit_http::model::{Currency, InstrumentKind, SortDirection};

    let currency: Currency = serde_json::from_value(serde_json::Value::String(
        input.currency.to_uppercase(),
    ))
    .map_err(|err| AdapterError::Validation {
        field: "currency".to_string(),
        message: err.to_string(),
    })?;

    let kind = match input.kind.as_deref() {
        None => None,
        Some(s) => Some(
            serde_json::from_value::<InstrumentKind>(serde_json::Value::String(s.to_lowercase()))
                .map_err(|err| AdapterError::Validation {
                field: "kind".to_string(),
                message: err.to_string(),
            })?,
        ),
    };

    let sorting = match input.sorting.as_deref() {
        None => None,
        Some(s) => Some(
            serde_json::from_value::<SortDirection>(serde_json::Value::String(s.to_lowercase()))
                .map_err(|err| AdapterError::Validation {
                    field: "sorting".to_string(),
                    message: err.to_string(),
                })?,
        ),
    };

    let request = deribit_http::model::request::trade::TradesRequest {
        currency,
        kind,
        start_id: input.start_id,
        end_id: input.end_id,
        count: input.count,
        start_timestamp: input.start_timestamp,
        end_timestamp: input.end_timestamp,
        sorting,
        historical: input.historical,
        subaccount_id: input.subaccount_id,
    };
    let result = ctx.http.get_user_trades_by_currency(request).await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- get_user_trades_by_instrument --------------------------------

/// `get_user_trades_by_instrument` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetUserTradesByInstrumentInput {
    /// Instrument identifier (`BTC-PERPETUAL`, …).
    pub instrument_name: String,
    /// First trade sequence number to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_seq: Option<u64>,
    /// Last trade sequence number to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_seq: Option<u64>,
    /// Page size (1..=1000; upstream default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// When `true`, include archived trades.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_old: Option<bool>,
    /// Sort direction: `asc` or `desc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sorting: Option<String>,
}

fn get_user_trades_by_instrument_tool() -> ToolEntry {
    let schema = schema_for::<GetUserTradesByInstrumentInput>();
    let descriptor = Tool::new(
        "get_user_trades_by_instrument",
        "User trades for a single instrument over a sequence-number window.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_get_user_trades_by_instrument(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Account,
        handler,
    }
}

async fn handle_get_user_trades_by_instrument(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: GetUserTradesByInstrumentInput = parse(input)?;
    let result = ctx
        .http
        .get_user_trades_by_instrument(
            &input.instrument_name,
            input.start_seq,
            input.end_seq,
            input.count,
            input.include_old,
            input.sorting.as_deref(),
        )
        .await?;
    Ok(serde_json::to_value(&result)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_account_tools_register_under_account_class() {
        for entry in [
            get_account_summary_tool(),
            get_positions_tool(),
            get_subaccounts_tool(),
            get_transaction_log_tool(),
            get_deposits_tool(),
            get_withdrawals_tool(),
            get_open_orders_by_currency_tool(),
            get_open_orders_by_instrument_tool(),
            get_user_trades_by_currency_tool(),
            get_user_trades_by_instrument_tool(),
        ] {
            assert_eq!(entry.class, ToolClass::Account);
        }
    }

    #[test]
    fn register_populates_full_account_set() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        let listed = registry.list();
        let names: Vec<&str> = listed.iter().map(|t| t.name.as_ref()).collect();
        for expected in [
            "get_account_summary",
            "get_deposits",
            "get_open_orders_by_currency",
            "get_open_orders_by_instrument",
            "get_positions",
            "get_subaccounts",
            "get_transaction_log",
            "get_user_trades_by_currency",
            "get_user_trades_by_instrument",
            "get_withdrawals",
        ] {
            assert!(
                names.contains(&expected),
                "missing tool {expected}; got {names:?}"
            );
        }
        assert_eq!(registry.len(), 10);
    }

    #[test]
    fn open_orders_by_currency_input_renames_type_field() {
        // The MCP schema field is named `type` (matching upstream),
        // even though the Rust field uses `order_type` to avoid the
        // reserved word. Round-trip a payload with `type` to pin the
        // mapping.
        let v = serde_json::json!({"currency": "BTC", "type": "limit"});
        let parsed: GetOpenOrdersByCurrencyInput = serde_json::from_value(v).expect("parse");
        assert_eq!(parsed.order_type.as_deref(), Some("limit"));
    }

    #[test]
    fn user_trades_by_instrument_input_accepts_required_only() {
        let v = serde_json::json!({"instrument_name": "BTC-PERPETUAL"});
        let parsed: GetUserTradesByInstrumentInput = serde_json::from_value(v).expect("parse");
        assert!(parsed.start_seq.is_none());
        assert!(parsed.end_seq.is_none());
    }

    #[test]
    fn transaction_log_input_requires_window() {
        let err =
            parse::<GetTransactionLogInput>(serde_json::json!({"currency": "BTC"})).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { .. }));
    }

    #[test]
    fn paginated_input_accepts_required_only() {
        let parsed: PaginatedCurrencyInput =
            serde_json::from_value(serde_json::json!({"currency": "BTC"})).expect("parse");
        assert!(parsed.count.is_none());
        assert!(parsed.offset.is_none());
    }

    #[test]
    fn account_summary_input_requires_currency() {
        let err = parse::<GetAccountSummaryInput>(serde_json::json!({})).unwrap_err();
        match err {
            AdapterError::Validation { field, .. } => assert_eq!(field, "arguments"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn positions_input_accepts_no_filters() {
        let parsed: GetPositionsInput =
            serde_json::from_value(serde_json::json!({})).expect("parse");
        assert!(parsed.currency.is_none());
        assert!(parsed.kind.is_none());
        assert!(parsed.subaccount_id.is_none());
    }

    #[test]
    fn subaccounts_input_accepts_no_arguments() {
        let parsed: GetSubaccountsInput =
            serde_json::from_value(serde_json::json!({})).expect("parse");
        assert!(parsed.with_portfolio.is_none());
    }
}
