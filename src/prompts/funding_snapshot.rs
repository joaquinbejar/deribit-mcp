//! `funding_snapshot` prompt (v0.5-03).
//!
//! Assembles a current + historical funding-rate snapshot for the
//! configured currency. Composes the public Read tools
//! `list_instruments` (filtered to the perpetual) and
//! `get_funding_rate_history` over a caller-specified lookback
//! window.

use std::sync::Arc;

use rmcp::model::{
    GetPromptResult, JsonObject, Prompt, PromptArgument, PromptMessage, PromptMessageContent,
    PromptMessageRole,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PromptEntry, PromptHandlerFn, PromptRegistry};
use crate::error::AdapterError;

/// Prompt name as it appears on `prompts/list`.
pub const NAME: &str = "funding_snapshot";

/// Maximum lookback the prompt accepts. Wider windows should be
/// chunked into separate runs so the produced prompt stays
/// bounded and the upstream `get_funding_rate_history` request
/// stays under Deribit's documented limit.
const MAX_LOOKBACK_HOURS: u32 = 720; // 30 days.

/// `funding_snapshot` arguments.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct FundingSnapshotInput {
    /// Currency to snapshot. `BTC` or `ETH` are the documented
    /// perpetual-funding currencies.
    pub currency: String,
    /// How far back, in hours, the historical sample should reach.
    /// Range: `1..=720` (1 hour up to 30 days).
    pub lookback_hours: u32,
}

/// Register the prompt.
pub fn register(registry: &mut PromptRegistry) {
    registry.insert(entry());
}

fn descriptor() -> Prompt {
    let arguments = vec![
        PromptArgument::new("currency")
            .with_description(
                "`BTC` or `ETH`. Currency whose perpetual funding will be snapshotted.",
            )
            .with_required(true),
        PromptArgument::new("lookback_hours")
            .with_description(
                "How many hours of funding history to include. Integer in the range \
                 1..=720 (30 days); values outside the range are rejected.",
            )
            .with_required(true),
    ];
    Prompt::new(
        NAME,
        Some(
            "Compose a current + historical funding-rate snapshot for the configured \
             currency. Names `list_instruments` + `get_funding_rate_history` and pins \
             the tabular output shape.",
        ),
        Some(arguments),
    )
}

fn entry() -> PromptEntry {
    let handler: PromptHandlerFn = Arc::new(|_ctx, args| Box::pin(render(args)));
    PromptEntry {
        descriptor: descriptor(),
        handler,
    }
}

async fn render(args: JsonObject) -> Result<GetPromptResult, AdapterError> {
    let input: FundingSnapshotInput = parse_args(args)?;
    let normalized = normalize_currency(&input.currency)?;
    let lookback = validate_lookback(input.lookback_hours)?;
    let instrument = format!("{normalized}-PERPETUAL");

    let user_text = format!(
        "Produce a {normalized} perpetual funding snapshot covering the last \
         {lookback} hour(s). Use ONLY the following Read tools:\n\
         \n\
         1. `list_instruments {{ currency: \"{normalized}\", kind: \"future\", \
         expired: false }}` — confirm `{instrument}` is the active perpetual.\n\
         2. `get_funding_rate_history {{ instrument_name: \"{instrument}\", \
         start_timestamp: now_ms - {lookback} * 3_600_000, end_timestamp: now_ms }}` \
         — pull the per-8h funding samples.\n\
         \n\
         Final output (in this order):\n\
         - **Latest** funding rate, 8h-annualised, with its timestamp.\n\
         - **Mean / median / p10 / p90** funding over the lookback window.\n\
         - **Sign breakdown**: how many samples positive vs. negative.\n\
         - **Notable outliers**: any sample > 2σ from the window mean, with \
         timestamp and value.\n\
         - **Caveats**: clock skew between `now_ms` and the upstream timestamps; \
         any gap in the returned series."
    );

    let assistant_ack = format!(
        "Acknowledged. I will assemble a {normalized} funding snapshot over the last \
         {lookback} hour(s) using only the listed Read tools."
    );

    let messages = vec![
        PromptMessage::new(
            PromptMessageRole::User,
            PromptMessageContent::Text { text: user_text },
        ),
        PromptMessage::new(
            PromptMessageRole::Assistant,
            PromptMessageContent::Text {
                text: assistant_ack,
            },
        ),
    ];
    Ok(GetPromptResult::new(messages).with_description(format!(
        "Funding snapshot for {normalized} (lookback: {lookback} hour(s))"
    )))
}

fn parse_args(args: JsonObject) -> Result<FundingSnapshotInput, AdapterError> {
    serde_json::from_value(serde_json::Value::Object(args)).map_err(|err| {
        AdapterError::Validation {
            field: "arguments".to_string(),
            message: err.to_string(),
        }
    })
}

fn normalize_currency(s: &str) -> Result<String, AdapterError> {
    let upper = s.trim().to_ascii_uppercase();
    if upper == "BTC" || upper == "ETH" {
        Ok(upper)
    } else {
        Err(AdapterError::Validation {
            field: "currency".to_string(),
            message: format!("expected `BTC` or `ETH`, got `{s}`"),
        })
    }
}

fn validate_lookback(hours: u32) -> Result<u32, AdapterError> {
    if (1..=MAX_LOOKBACK_HOURS).contains(&hours) {
        Ok(hours)
    } else {
        Err(AdapterError::Validation {
            field: "lookback_hours".to_string(),
            message: format!("expected 1..={MAX_LOOKBACK_HOURS}, got {hours}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(value: serde_json::Value) -> JsonObject {
        match value {
            serde_json::Value::Object(map) => map,
            _ => panic!("test input must be JSON object"),
        }
    }

    #[tokio::test]
    async fn render_returns_two_messages() {
        let r = render(args(json!({"currency":"BTC","lookback_hours":24})))
            .await
            .unwrap();
        assert_eq!(r.messages.len(), 2);
        let PromptMessageContent::Text { ref text } = r.messages[0].content else {
            panic!("text expected");
        };
        assert!(text.contains("BTC-PERPETUAL"));
        assert!(text.contains("24 hour(s)"));
    }

    #[tokio::test]
    async fn render_rejects_unknown_currency() {
        let err = render(args(json!({"currency":"XRP","lookback_hours":24})))
            .await
            .unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "currency"));
    }

    #[tokio::test]
    async fn render_rejects_zero_lookback() {
        let err = render(args(json!({"currency":"BTC","lookback_hours":0})))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AdapterError::Validation { ref field, .. } if field == "lookback_hours")
        );
    }

    #[tokio::test]
    async fn render_rejects_lookback_over_cap() {
        let err = render(args(json!({"currency":"BTC","lookback_hours":721})))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AdapterError::Validation { ref field, .. } if field == "lookback_hours")
        );
    }
}
