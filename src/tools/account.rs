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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_account_tools_register_under_account_class() {
        for entry in [
            get_account_summary_tool(),
            get_positions_tool(),
            get_subaccounts_tool(),
        ] {
            assert_eq!(entry.class, ToolClass::Account);
        }
    }

    #[test]
    fn register_populates_three_tools() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        let listed = registry.list();
        let names: Vec<&str> = listed.iter().map(|t| t.name.as_ref()).collect();
        for expected in ["get_account_summary", "get_positions", "get_subaccounts"] {
            assert!(
                names.contains(&expected),
                "missing tool {expected}; got {names:?}"
            );
        }
        assert_eq!(registry.len(), 3);
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
