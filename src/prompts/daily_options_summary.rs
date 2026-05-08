//! `daily_options_summary` prompt (v0.5-02).
//!
//! Walks the LLM through "summarise BTC / ETH options expiring in
//! the next N days". Composes the existing public Read tools
//! (`list_instruments`, `get_book_summary_by_currency`,
//! `get_historical_volatility`).
//!
//! The prompt itself does NOT call those tools. It returns a User
//! message that names them and constrains how the LLM should weave
//! their output into the final summary, plus an Assistant
//! acknowledgement that pins the agreed shape.

use std::sync::Arc;

use rmcp::model::{
    GetPromptResult, JsonObject, Prompt, PromptArgument, PromptMessage, PromptMessageContent,
    PromptMessageRole,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PromptEntry, PromptHandlerFn, PromptRegistry};
use crate::error::AdapterError;

/// Prompt name as it appears on `prompts/list`. `pub` so the
/// integration suite can refer to it by symbol rather than
/// duplicating the string literal.
pub const NAME: &str = "daily_options_summary";

/// `daily_options_summary` arguments — typed view of the
/// `prompts/get` `arguments` payload.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct DailyOptionsSummaryInput {
    /// Currency to summarise. `BTC` or `ETH` are the documented
    /// options-bearing currencies on Deribit; other strings are
    /// rejected.
    pub currency: String,
    /// How far ahead, in days, the summary should reach. Capped at
    /// 31 to keep the produced prompt bounded — wider windows
    /// should be split into separate runs.
    pub horizon_days: u8,
}

/// Register the prompt into the shared [`PromptRegistry`].
pub fn register(registry: &mut PromptRegistry) {
    registry.insert(entry());
}

fn descriptor() -> Prompt {
    let arguments = vec![
        PromptArgument::new("currency")
            .with_description("`BTC` or `ETH`. The base currency to summarise.")
            .with_required(true),
        PromptArgument::new("horizon_days")
            .with_description(
                "How many days ahead the summary should reach. Integer in the range \
                 1..=31; values outside the range are rejected.",
            )
            .with_required(true),
    ];
    Prompt::new(
        NAME,
        Some(
            "Summarise BTC / ETH options expiring within the requested horizon \
             using the public Read tools (`list_instruments`, \
             `get_book_summary_by_currency`, `get_historical_volatility`).",
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
    let input: DailyOptionsSummaryInput = parse_args(args)?;
    let normalized = normalize_currency(&input.currency)?;
    let horizon = validate_horizon(input.horizon_days)?;

    let user_message = format!(
        "Produce a structured summary of {normalized} options expiring within \
         {horizon} day(s). Use ONLY the following Read tools, in this order:\n\
         \n\
         1. `list_instruments {{ currency: \"{normalized}\", kind: \"option\" }}` — \
         filter the results client-side to instruments whose `expiration_timestamp` \
         falls within the next {horizon} day(s).\n\
         2. `get_book_summary_by_currency {{ currency: \"{normalized}\", kind: \
         \"option\" }}` — pull mid prices, open interest, and 24h volume for the \
         filtered set.\n\
         3. `get_historical_volatility {{ currency: \"{normalized}\" }}` — anchor \
         IV vs. RV in the headline.\n\
         \n\
         Final output sections (in this order):\n\
         - Headline: ATM straddle implied vs. realised volatility.\n\
         - Top three calls and top three puts by open interest, with strike, \
         expiration, mid price, and 24h volume.\n\
         - Skew commentary: any expiry where the 25-delta risk reversal looks \
         dislocated.\n\
         - Caveats: timestamps for every figure, instruments excluded for \
         missing data."
    );

    let assistant_ack = format!(
        "Acknowledged. I will summarise {normalized} options within {horizon} day(s) \
         using only the listed Read tools, then return the four-section structured \
         output."
    );

    let messages = vec![
        PromptMessage::new(
            PromptMessageRole::User,
            PromptMessageContent::Text { text: user_message },
        ),
        PromptMessage::new(
            PromptMessageRole::Assistant,
            PromptMessageContent::Text {
                text: assistant_ack,
            },
        ),
    ];
    Ok(GetPromptResult::new(messages).with_description(format!(
        "Daily options summary for {normalized} (horizon: {horizon} day(s))"
    )))
}

fn parse_args(args: JsonObject) -> Result<DailyOptionsSummaryInput, AdapterError> {
    let value = serde_json::Value::Object(args);
    serde_json::from_value(value).map_err(|err| AdapterError::Validation {
        field: "arguments".to_string(),
        message: err.to_string(),
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

fn validate_horizon(horizon_days: u8) -> Result<u8, AdapterError> {
    if (1..=31).contains(&horizon_days) {
        Ok(horizon_days)
    } else {
        Err(AdapterError::Validation {
            field: "horizon_days".to_string(),
            message: format!("expected 1..=31, got {horizon_days}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args_obj(value: serde_json::Value) -> JsonObject {
        match value {
            serde_json::Value::Object(map) => map,
            _ => panic!("test input must be JSON object"),
        }
    }

    #[tokio::test]
    async fn render_returns_two_messages_for_btc_7d() {
        let result = render(args_obj(json!({"currency":"BTC","horizon_days":7})))
            .await
            .unwrap();
        assert_eq!(result.messages.len(), 2);
        assert!(matches!(result.messages[0].role, PromptMessageRole::User));
        assert!(matches!(
            result.messages[1].role,
            PromptMessageRole::Assistant
        ));
        let PromptMessageContent::Text { ref text } = result.messages[0].content else {
            panic!("expected text content");
        };
        assert!(text.contains("BTC options"));
        assert!(text.contains("7 day(s)"));
    }

    #[tokio::test]
    async fn render_normalises_currency_case() {
        let result = render(args_obj(json!({"currency":"eth","horizon_days":1})))
            .await
            .unwrap();
        let PromptMessageContent::Text { ref text } = result.messages[0].content else {
            panic!("expected text content");
        };
        assert!(text.contains("ETH options"));
    }

    #[tokio::test]
    async fn render_rejects_unknown_currency() {
        let err = render(args_obj(json!({"currency":"XRP","horizon_days":7})))
            .await
            .unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "currency"));
    }

    #[tokio::test]
    async fn render_rejects_zero_horizon() {
        let err = render(args_obj(json!({"currency":"BTC","horizon_days":0})))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AdapterError::Validation { ref field, .. } if field == "horizon_days")
        );
    }

    #[tokio::test]
    async fn render_rejects_horizon_over_cap() {
        let err = render(args_obj(json!({"currency":"BTC","horizon_days":99})))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AdapterError::Validation { ref field, .. } if field == "horizon_days")
        );
    }

    #[tokio::test]
    async fn render_rejects_missing_currency() {
        let err = render(args_obj(json!({"horizon_days":7})))
            .await
            .unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "arguments"));
    }

    #[test]
    fn descriptor_matches_documented_name_and_args() {
        let d = descriptor();
        assert_eq!(d.name, NAME);
        let args = d.arguments.expect("arguments");
        let names: Vec<_> = args.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["currency", "horizon_days"]);
        assert!(args.iter().all(|a| a.required == Some(true)));
    }
}
