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
            "get_positions",
            "get_subaccounts",
            "get_transaction_log",
            "get_withdrawals",
        ] {
            assert!(
                names.contains(&expected),
                "missing tool {expected}; got {names:?}"
            );
        }
        assert_eq!(registry.len(), 6);
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
