//! `position_review` prompt (v0.5-04).
//!
//! Drives the LLM through an end-of-day account / position review
//! using the `Account` tools shipped in v0.2. The prompt itself is
//! always registerable; whether the LLM can fulfil it depends on
//! whether credentials are configured. With credentials present
//! the prompt names the four `Account` tools the LLM should run;
//! without credentials it returns a structured *warning* note so
//! the LLM stops instead of trying to call tools that are not in
//! `tools/list`.

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
pub const NAME: &str = "position_review";

/// `position_review` arguments.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct PositionReviewInput {
    /// Currency to scope the review to (`BTC`, `ETH`, `USDC`, …).
    pub currency: String,
    /// When `true`, include a recent-trades section by calling
    /// `get_user_trades_by_currency`. Defaults to `false`.
    #[serde(default)]
    pub include_history: bool,
}

/// Register the prompt.
pub fn register(registry: &mut PromptRegistry) {
    registry.insert(entry());
}

fn descriptor() -> Prompt {
    let arguments = vec![
        PromptArgument::new("currency")
            .with_description("Currency to scope the review to (`BTC`, `ETH`, `USDC`, …).")
            .with_required(true),
        PromptArgument::new("include_history")
            .with_description("When `true`, append a recent-trades section. Defaults to `false`.")
            .with_required(false),
    ];
    Prompt::new(
        NAME,
        Some(
            "Drive the LLM through an account / position review using the v0.2 \
             `Account` tools (get_account_summary, get_positions, \
             get_open_orders_by_currency, get_user_trades_by_currency). When \
             credentials are not configured, returns a structured warning instead.",
        ),
        Some(arguments),
    )
}

fn entry() -> PromptEntry {
    let handler: PromptHandlerFn =
        Arc::new(|ctx, args| Box::pin(render(ctx.has_credentials(), args)));
    PromptEntry {
        descriptor: descriptor(),
        handler,
    }
}

async fn render(has_credentials: bool, args: JsonObject) -> Result<GetPromptResult, AdapterError> {
    let input: PositionReviewInput = parse_args(args)?;
    let currency = normalize_currency(&input.currency)?;

    if !has_credentials {
        return Ok(missing_credentials_response(&currency));
    }

    let history_step = if input.include_history {
        format!(
            "4. `get_user_trades_by_currency {{ currency: \"{currency}\", count: 50 }}` — \
             pull the last 50 user trades for context.\n"
        )
    } else {
        String::new()
    };

    let user_text = format!(
        "Produce an end-of-day {currency} position review. Use ONLY the following \
         Account tools, in this order:\n\
         \n\
         1. `get_account_summary {{ currency: \"{currency}\", extended: true }}` — \
         pull equity, margin balance, initial / maintenance margin, P&L.\n\
         2. `get_positions {{ currency: \"{currency}\" }}` — every open position for \
         the currency.\n\
         3. `get_open_orders_by_currency {{ currency: \"{currency}\" }}` — every \
         resting order for the currency.\n\
         {history_step}\
         \n\
         Final output sections (in this order):\n\
         - **Headline**: equity, P&L (24h + session), margin utilisation %.\n\
         - **Positions**: instrument, side, size, mark price, unrealised P&L, \
         liquidation price (when present).\n\
         - **Open orders**: instrument, side, type, price, amount, age in minutes.\n\
         - **Flags**: any position with margin utilisation > 70%, any reduce-only \
         order on a position the wrong way, any order older than 24h.\n\
         {history_section}\
         - **Caveats**: timestamps for every figure, any tool that returned a \
         partial / errored response.",
        history_step = history_step,
        history_section = if input.include_history {
            "- **Recent trades**: top 5 by notional from the last 50 trades, with \
             instrument, side, price, amount, fee.\n"
        } else {
            ""
        }
    );

    let assistant_ack = format!(
        "Acknowledged. I will run a {currency} position review using only the \
         listed Account tools and return the structured sections."
    );

    Ok(GetPromptResult::new(vec![
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
    ])
    .with_description(format!("Position review for {currency}")))
}

/// Build the no-credentials response. The prompt body explains in
/// natural-language *and* tags the body with a `WARNING:` prefix so
/// the LLM knows not to attempt tool calls that are not in
/// `tools/list`.
fn missing_credentials_response(currency: &str) -> GetPromptResult {
    let user_text = format!(
        "WARNING: this MCP server has no credentials configured. The Account tools \
         (`get_account_summary`, `get_positions`, `get_open_orders_by_currency`, \
         `get_user_trades_by_currency`) are NOT registered in `tools/list`, so a \
         {currency} position review cannot be produced.\n\
         \n\
         To enable this prompt, set `DERIBIT_CLIENT_ID` and `DERIBIT_CLIENT_SECRET` \
         in the server's environment and restart the binary.\n\
         \n\
         Reply with a brief explanation of the warning and stop."
    );
    let assistant_ack = format!(
        "Understood. The MCP server is anonymous; I cannot produce a {currency} \
         position review until credentials are configured. I will not attempt any \
         Account tool call."
    );
    GetPromptResult::new(vec![
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
    ])
    .with_description(format!(
        "Position review for {currency} — credentials missing"
    ))
}

fn parse_args(args: JsonObject) -> Result<PositionReviewInput, AdapterError> {
    serde_json::from_value(serde_json::Value::Object(args)).map_err(|err| {
        AdapterError::Validation {
            field: "arguments".to_string(),
            message: err.to_string(),
        }
    })
}

fn normalize_currency(s: &str) -> Result<String, AdapterError> {
    let upper = s.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return Err(AdapterError::Validation {
            field: "currency".to_string(),
            message: "must be non-empty".to_string(),
        });
    }
    Ok(upper)
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
    async fn render_with_credentials_lists_account_tools() {
        let r = render(true, args(json!({"currency":"BTC"}))).await.unwrap();
        assert_eq!(r.messages.len(), 2);
        let PromptMessageContent::Text { ref text } = r.messages[0].content else {
            panic!("text expected");
        };
        for tool in [
            "get_account_summary",
            "get_positions",
            "get_open_orders_by_currency",
        ] {
            assert!(text.contains(tool), "expected {tool} in body");
        }
        // include_history defaults false → no get_user_trades_by_currency.
        assert!(!text.contains("get_user_trades_by_currency"));
    }

    #[tokio::test]
    async fn render_with_history_includes_user_trades_step() {
        let r = render(true, args(json!({"currency":"ETH","include_history":true})))
            .await
            .unwrap();
        let PromptMessageContent::Text { ref text } = r.messages[0].content else {
            panic!("text expected");
        };
        assert!(text.contains("get_user_trades_by_currency"));
        assert!(text.contains("ETH"));
    }

    #[tokio::test]
    async fn render_without_credentials_emits_warning() {
        let r = render(false, args(json!({"currency":"BTC"})))
            .await
            .unwrap();
        let PromptMessageContent::Text { ref text } = r.messages[0].content else {
            panic!("text expected");
        };
        assert!(text.starts_with("WARNING:"));
        assert!(text.contains("DERIBIT_CLIENT_ID"));
        // Warning body lists the missing tools by name but does NOT
        // emit the "Use ONLY the following Account tools" call list.
        assert!(!text.contains("Use ONLY the following Account tools"));
        assert!(!text.contains("Final output sections"));
    }

    #[tokio::test]
    async fn render_rejects_empty_currency() {
        let err = render(true, args(json!({"currency":"   "})))
            .await
            .unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "currency"));
    }

    #[test]
    fn descriptor_matches_documented_name_and_args() {
        let d = descriptor();
        assert_eq!(d.name, NAME);
        let args = d.arguments.expect("arguments");
        let names: Vec<_> = args.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["currency", "include_history"]);
        assert_eq!(args[0].required, Some(true));
        assert_eq!(args[1].required, Some(false));
    }
}
