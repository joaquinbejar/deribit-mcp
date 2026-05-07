//! `Trading` tool family.
//!
//! All tools in this module have [`ToolClass::Trading`]. The registry
//! refuses to register them unless **both** `DERIBIT_CLIENT_ID` /
//! `DERIBIT_CLIENT_SECRET` credentials are configured **and**
//! `--allow-trading` is passed (ADR-0010); `--max-order-usd` caps
//! notional size in a future v0.4-04 step.
//!
//! v0.4-01 ships:
//!
//! - `place_order` — buy / sell with the full Deribit order parameter
//!   surface (limit / market / stop / take, optional time-in-force,
//!   trigger, post-only / reduce-only / mmp flags, `valid_until`).
//!
//! [`ToolClass::Trading`]: super::ToolClass::Trading

use std::sync::Arc;

use rmcp::model::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::schema::{parse_input as parse, schema_for};
use super::{ToolClass, ToolEntry, ToolHandlerFn, ToolRegistry};
use crate::context::AdapterContext;
use crate::error::AdapterError;

/// Register every `Trading` tool with the registry.
pub fn register(registry: &mut ToolRegistry) {
    registry.insert(place_order_tool());
    registry.insert(edit_order_tool());
    registry.insert(cancel_order_tool());
    registry.insert(cancel_all_by_currency_tool());
    registry.insert(cancel_all_by_instrument_tool());
}

// ----- place_order ---------------------------------------------------

/// Buy or sell.
///
/// Closed-set enum kept local to the adapter so the JSON wire shape
/// (`"buy"` / `"sell"`) stays stable independent of any future
/// rename in the upstream `deribit-http` `OrderSide`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Buy.
    Buy,
    /// Sell.
    Sell,
}

/// Order type accepted by `place_order`.
///
/// One-to-one with the upstream `deribit_http::model::order::OrderType`
/// variants the adapter supports. `TrailingStop` is not exposed yet —
/// it lacks adapter-side validation rules and ships once v0.4 stabilises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaceOrderType {
    /// Limit order. Requires `price`.
    Limit,
    /// Market order.
    Market,
    /// Stop limit. Requires `price`, `trigger_price`, `trigger`.
    StopLimit,
    /// Stop market. Requires `trigger_price`, `trigger`.
    StopMarket,
    /// Take-profit limit. Requires `price`, `trigger_price`, `trigger`.
    TakeLimit,
    /// Take-profit market. Requires `trigger_price`, `trigger`.
    TakeMarket,
    /// Market-limit order — market with limit-price protection.
    MarketLimit,
}

/// Time-in-force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaceTimeInForce {
    /// Default. Resting until cancelled.
    GoodTilCancelled,
    /// Expires at end of trading day.
    GoodTilDay,
    /// Fully fill immediately or cancel.
    FillOrKill,
    /// Fill what is possible immediately, cancel the rest.
    ImmediateOrCancel,
}

/// Trigger reference price for stop / take variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaceTrigger {
    /// Index price.
    IndexPrice,
    /// Mark price.
    MarkPrice,
    /// Last traded price.
    LastPrice,
}

/// `place_order` input.
///
/// Mirrors the parameters Deribit's `/private/buy` and
/// `/private/sell` endpoints accept. Adapter-side validation runs
/// before the upstream call: required-fields-by-order-type, finite
/// numeric values, strictly positive `amount` / `price`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct PlaceOrderInput {
    /// Instrument identifier (`BTC-PERPETUAL`, `ETH-29SEP23-1500-C`,
    /// …). Routed to Deribit verbatim.
    pub instrument_name: String,
    /// `buy` or `sell`. Decides which upstream endpoint runs.
    pub side: Side,
    /// Order amount. Finite, `> 0`. Contracts for options,
    /// quote-currency for futures / perpetuals (Deribit's own
    /// convention — the adapter does not reinterpret it).
    pub amount: f64,
    /// Order type. Required-field rules:
    /// - `limit` / `stop_limit` / `take_limit` → `price` is required.
    /// - `stop_*` / `take_*` → `trigger_price` and `trigger` required.
    #[serde(rename = "type")]
    pub order_type: PlaceOrderType,
    /// Limit price. Required for limit / stop_limit / take_limit.
    /// Finite, `> 0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    /// Time-in-force override. Defaults upstream to `good_til_cancelled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_in_force: Option<PlaceTimeInForce>,
    /// User-defined label echoed back on the resulting order; max 64
    /// chars upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Trigger price. Required for stop / take variants. Finite, `> 0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_price: Option<f64>,
    /// Trigger reference. Required alongside `trigger_price`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<PlaceTrigger>,
    /// Post-only flag — reject the order if it would cross the book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_only: Option<bool>,
    /// `reject_post_only` — reject (vs. amend) when `post_only`
    /// would otherwise be relaxed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_post_only: Option<bool>,
    /// Reduce-only — only decrease, never flip, the position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduce_only: Option<bool>,
    /// Market-Maker Protection flag (advanced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmp: Option<bool>,
    /// Server-side TTL (Unix-ms). After this timestamp the order is
    /// auto-cancelled. Unsigned to match the rest of the adapter's
    /// timestamp inputs (`start_timestamp` / `end_timestamp` in the
    /// Account family).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<u64>,
}

fn place_order_tool() -> ToolEntry {
    let schema = schema_for::<PlaceOrderInput>();
    let descriptor = Tool::new(
        "place_order",
        "Place a buy or sell order. This sends a real order to the configured Deribit endpoint.",
        schema,
    );
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_place_order(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Trading,
        handler,
    }
}

async fn handle_place_order(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: PlaceOrderInput = parse(input)?;
    validate_place_order(&input)?;
    enforce_size_cap(ctx, &input.instrument_name, input.amount, input.price).await?;
    let (side, request) = build_order_request(input)?;
    let result = match side {
        Side::Buy => ctx.http.buy_order(request).await?,
        Side::Sell => ctx.http.sell_order(request).await?,
    };
    Ok(serde_json::to_value(&result)?)
}

/// Enforce the per-process notional cap configured via
/// `--max-order-usd` / `DERIBIT_MAX_ORDER_USD`.
///
/// Runs after schema validation, before any private
/// order-placement call (`/private/buy` / `/private/sell`). When
/// the cap is unset the function is a no-op. When set, it
/// converts the order size to a USD-equivalent notional and
/// rejects the call with [`AdapterError::SizeCapExceeded`] if the
/// notional is over the cap. May fetch upstream pricing data
/// (`/public/ticker`, `/public/get_index_price`) when the caller
/// did not supply a `price` or the instrument is an option.
///
/// # Notional formula
///
/// Classifies the instrument by its name suffix:
///
/// - **Linear** (`*_USDC*` / `*_USDT*` — settlement in stablecoin):
///   `notional = amount × price`. For market orders without a
///   provided `price`, the upstream `mark_price` from
///   `/public/ticker` is used.
/// - **Option** (final segment is `-C` / `-P` with a numeric
///   strike): `notional = amount × index_price`, where
///   `index_price` is the upstream USD index for the option's
///   underlying (`BTC`, `ETH`, …) via `/public/get_index_price`.
///   This pins the conservative upper bound of underlying USD
///   exposure for `amount` contracts of the option.
/// - **Inverse** (everything else — BTC- / ETH-denominated
///   futures and perpetuals): `amount` is already USD-notional
///   per Deribit's contract-size convention, so the notional is
///   `amount` directly.
///
/// The classification is conservative — when in doubt the inverse
/// branch is taken so the cap doesn't silently let through a
/// larger order.
///
/// # Errors
///
/// Returns [`AdapterError::SizeCapExceeded`] when the computed
/// notional exceeds the cap. May also propagate
/// [`AdapterError::Upstream`] if a price-fetch needed for the
/// notional calculation fails.
pub(crate) async fn enforce_size_cap(
    ctx: &AdapterContext,
    instrument_name: &str,
    amount: f64,
    price: Option<f64>,
) -> Result<(), AdapterError> {
    let Some(cap) = ctx.config.max_order_usd else {
        return Ok(());
    };
    let cap = cap as f64;
    let notional = compute_usd_notional(ctx, instrument_name, amount, price).await?;
    if !notional.is_finite() {
        return Err(AdapterError::Validation {
            field: "amount".to_string(),
            message: "computed notional is not finite".to_string(),
        });
    }
    if notional > cap {
        return Err(AdapterError::SizeCapExceeded {
            requested: notional,
            cap,
        });
    }
    Ok(())
}

/// `true` when the instrument is linear (USDC / USDT settlement).
fn is_linear_instrument(name: &str) -> bool {
    name.contains("_USDC") || name.contains("_USDT")
}

/// `true` when the instrument is an option — last segment is a
/// single `C` or `P` and the segment before it parses as a
/// strike. Matches Deribit's `<BASE>-<EXPIRY>-<STRIKE>-<C|P>`
/// pattern.
fn is_option_instrument(name: &str) -> bool {
    let mut parts = name.rsplit('-');
    let Some(last) = parts.next() else {
        return false;
    };
    if last != "C" && last != "P" {
        return false;
    }
    let Some(strike) = parts.next() else {
        return false;
    };
    strike.parse::<f64>().is_ok()
}

/// Underlying base symbol for an option instrument
/// (`BTC-29SEP24-50000-C` → `Some("BTC")`).
fn option_underlying_base(name: &str) -> Option<&str> {
    name.split('-').next()
}

async fn compute_usd_notional(
    ctx: &AdapterContext,
    instrument_name: &str,
    amount: f64,
    price: Option<f64>,
) -> Result<f64, AdapterError> {
    if is_option_instrument(instrument_name) {
        // Conservative upper bound of underlying USD exposure for
        // `amount` contracts of the option. Each contract is for
        // one unit of the underlying at Deribit, so the underlying
        // notional is `amount × index_price`.
        let base = option_underlying_base(instrument_name).unwrap_or("BTC");
        let index_name = format!("{}_usd", base.to_lowercase());
        let idx = ctx.http.get_index_price(&index_name).await?;
        return Ok(amount * idx.index_price);
    }
    if !is_linear_instrument(instrument_name) {
        // Inverse — amount is USD notional directly.
        return Ok(amount);
    }
    let p = match price {
        Some(p) => p,
        None => {
            let ticker = ctx.http.get_ticker(instrument_name).await?;
            ticker.mark_price
        }
    };
    Ok(amount * p)
}

/// Adapter-side validation ahead of any network call.
///
/// # Errors
///
/// Returns [`AdapterError::Validation`] when:
///
/// - `amount` is non-finite or `<= 0`.
/// - `price` is supplied non-finite or `<= 0`.
/// - `trigger_price` is supplied non-finite or `<= 0`.
/// - The order type requires `price` and it is missing.
/// - The order type requires `trigger_price` / `trigger` and they
///   are missing.
fn validate_place_order(input: &PlaceOrderInput) -> Result<(), AdapterError> {
    if !input.amount.is_finite() || input.amount <= 0.0 {
        return Err(AdapterError::Validation {
            field: "amount".to_string(),
            message: format!("must be finite and > 0, got {}", input.amount),
        });
    }
    if let Some(price) = input.price
        && (!price.is_finite() || price <= 0.0)
    {
        return Err(AdapterError::Validation {
            field: "price".to_string(),
            message: format!("must be finite and > 0, got {price}"),
        });
    }
    if let Some(tp) = input.trigger_price
        && (!tp.is_finite() || tp <= 0.0)
    {
        return Err(AdapterError::Validation {
            field: "trigger_price".to_string(),
            message: format!("must be finite and > 0, got {tp}"),
        });
    }
    let needs_price = matches!(
        input.order_type,
        PlaceOrderType::Limit | PlaceOrderType::StopLimit | PlaceOrderType::TakeLimit
    );
    if needs_price && input.price.is_none() {
        return Err(AdapterError::Validation {
            field: "price".to_string(),
            message: "required for limit / stop_limit / take_limit orders".to_string(),
        });
    }
    let needs_trigger = matches!(
        input.order_type,
        PlaceOrderType::StopLimit
            | PlaceOrderType::StopMarket
            | PlaceOrderType::TakeLimit
            | PlaceOrderType::TakeMarket
    );
    if needs_trigger {
        if input.trigger_price.is_none() {
            return Err(AdapterError::Validation {
                field: "trigger_price".to_string(),
                message: "required for stop / take order types".to_string(),
            });
        }
        if input.trigger.is_none() {
            return Err(AdapterError::Validation {
                field: "trigger".to_string(),
                message: "required for stop / take order types".to_string(),
            });
        }
    }
    Ok(())
}

/// Build the upstream `OrderRequest` plus the side-tag controlling
/// which endpoint to hit. Closed-set matches on `PlaceOrderType` /
/// `PlaceTimeInForce` / `PlaceTrigger` keep the conversion total.
///
/// # Errors
///
/// Returns [`AdapterError::Validation`] if `valid_until` overflows
/// `i64` — practically unreachable for any real Unix-ms timestamp,
/// but the upstream `OrderRequest.valid_until` is signed so the
/// cast is checked here at the boundary instead of silently wrapping.
fn build_order_request(
    input: PlaceOrderInput,
) -> Result<(Side, deribit_http::model::request::OrderRequest), AdapterError> {
    use deribit_http::model::order::OrderType as UpType;
    use deribit_http::model::request::OrderRequest;
    use deribit_http::model::trigger::Trigger as UpTrigger;
    use deribit_http::model::types::TimeInForce as UpTif;

    let order_type = match input.order_type {
        PlaceOrderType::Limit => UpType::Limit,
        PlaceOrderType::Market => UpType::Market,
        PlaceOrderType::StopLimit => UpType::StopLimit,
        PlaceOrderType::StopMarket => UpType::StopMarket,
        PlaceOrderType::TakeLimit => UpType::TakeLimit,
        PlaceOrderType::TakeMarket => UpType::TakeMarket,
        PlaceOrderType::MarketLimit => UpType::MarketLimit,
    };
    let time_in_force = input.time_in_force.map(|t| match t {
        PlaceTimeInForce::GoodTilCancelled => UpTif::GoodTilCancelled,
        PlaceTimeInForce::GoodTilDay => UpTif::GoodTilDay,
        PlaceTimeInForce::FillOrKill => UpTif::FillOrKill,
        PlaceTimeInForce::ImmediateOrCancel => UpTif::ImmediateOrCancel,
    });
    let trigger = input.trigger.map(|t| match t {
        PlaceTrigger::IndexPrice => UpTrigger::IndexPrice,
        PlaceTrigger::MarkPrice => UpTrigger::MarkPrice,
        PlaceTrigger::LastPrice => UpTrigger::LastPrice,
    });
    let valid_until = match input.valid_until {
        None => None,
        Some(v) => Some(i64::try_from(v).map_err(|_| AdapterError::Validation {
            field: "valid_until".to_string(),
            message: format!("must fit in i64, got {v}"),
        })?),
    };
    let req = OrderRequest {
        order_id: None,
        instrument_name: input.instrument_name,
        amount: Some(input.amount),
        contracts: None,
        type_: Some(order_type),
        label: input.label,
        price: input.price,
        time_in_force,
        display_amount: None,
        post_only: input.post_only,
        reject_post_only: input.reject_post_only,
        reduce_only: input.reduce_only,
        trigger_price: input.trigger_price,
        trigger_offset: None,
        trigger,
        advanced: None,
        mmp: input.mmp,
        valid_until,
        linked_order_type: None,
        trigger_fill_condition: None,
        otoco_config: None,
    };
    Ok((input.side, req))
}

// ----- edit_order ---------------------------------------------------

/// `edit_order` input.
///
/// Modifies an existing order identified by `order_id`. All other
/// fields are optional — `None` means "leave the existing value
/// unchanged upstream".
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct EditOrderInput {
    /// Order id returned from `place_order`.
    pub order_id: String,
    /// New amount. Finite, `> 0` if supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    /// New limit price. Finite, `> 0` if supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    /// Toggle post-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_only: Option<bool>,
    /// `reject_post_only` — reject (vs. amend) when `post_only`
    /// would otherwise be relaxed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_post_only: Option<bool>,
    /// Toggle reduce-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduce_only: Option<bool>,
    /// Toggle Market-Maker Protection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmp: Option<bool>,
    /// Server-side TTL (Unix-ms). `None` keeps the existing value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<u64>,
    /// New trigger price for stop / take orders. Finite, `> 0` if
    /// supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_price: Option<f64>,
}

fn edit_order_tool() -> ToolEntry {
    let schema = schema_for::<EditOrderInput>();
    let descriptor = Tool::new(
        "edit_order",
        "Modify amount / price / trigger_price / valid_until / flags (post_only, \
         reject_post_only, reduce_only, mmp) of an open order.",
        schema,
    );
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_edit_order(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Trading,
        handler,
    }
}

async fn handle_edit_order(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let mut input: EditOrderInput = parse(input)?;
    validate_edit_order(&input)?;
    // Forward the trimmed id — leading / trailing whitespace would
    // surface as an opaque upstream `OrderNotFound`.
    input.order_id = input.order_id.trim().to_string();
    let request = build_edit_request(input)?;
    let result = ctx.http.edit_order(request).await?;
    Ok(serde_json::to_value(&result)?)
}

fn validate_edit_order(input: &EditOrderInput) -> Result<(), AdapterError> {
    if input.order_id.trim().is_empty() {
        return Err(AdapterError::Validation {
            field: "order_id".to_string(),
            message: "must be non-empty".to_string(),
        });
    }
    let any_edit = input.amount.is_some()
        || input.price.is_some()
        || input.post_only.is_some()
        || input.reject_post_only.is_some()
        || input.reduce_only.is_some()
        || input.mmp.is_some()
        || input.valid_until.is_some()
        || input.trigger_price.is_some();
    if !any_edit {
        return Err(AdapterError::Validation {
            field: "arguments".to_string(),
            message: "at least one editable field must be present (amount, price, post_only, \
                      reject_post_only, reduce_only, mmp, valid_until, trigger_price)"
                .to_string(),
        });
    }
    if let Some(amount) = input.amount
        && (!amount.is_finite() || amount <= 0.0)
    {
        return Err(AdapterError::Validation {
            field: "amount".to_string(),
            message: format!("must be finite and > 0, got {amount}"),
        });
    }
    if let Some(price) = input.price
        && (!price.is_finite() || price <= 0.0)
    {
        return Err(AdapterError::Validation {
            field: "price".to_string(),
            message: format!("must be finite and > 0, got {price}"),
        });
    }
    if let Some(tp) = input.trigger_price
        && (!tp.is_finite() || tp <= 0.0)
    {
        return Err(AdapterError::Validation {
            field: "trigger_price".to_string(),
            message: format!("must be finite and > 0, got {tp}"),
        });
    }
    Ok(())
}

fn build_edit_request(
    input: EditOrderInput,
) -> Result<deribit_http::model::request::OrderRequest, AdapterError> {
    use deribit_http::model::request::OrderRequest;

    let valid_until = match input.valid_until {
        None => None,
        Some(v) => Some(i64::try_from(v).map_err(|_| AdapterError::Validation {
            field: "valid_until".to_string(),
            message: format!("must fit in i64, got {v}"),
        })?),
    };
    Ok(OrderRequest {
        order_id: Some(input.order_id),
        // `instrument_name` is unused by the upstream `edit_order`
        // path, but the struct field is non-optional. The empty
        // string round-trips harmlessly because `edit_order` only
        // serialises the fields it needs.
        instrument_name: String::new(),
        amount: input.amount,
        contracts: None,
        type_: None,
        label: None,
        price: input.price,
        time_in_force: None,
        display_amount: None,
        post_only: input.post_only,
        reject_post_only: input.reject_post_only,
        reduce_only: input.reduce_only,
        trigger_price: input.trigger_price,
        trigger_offset: None,
        trigger: None,
        advanced: None,
        mmp: input.mmp,
        valid_until,
        linked_order_type: None,
        trigger_fill_condition: None,
        otoco_config: None,
    })
}

// ----- cancel_order -------------------------------------------------

/// `cancel_order` input.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CancelOrderInput {
    /// Order id returned from `place_order`.
    pub order_id: String,
}

fn cancel_order_tool() -> ToolEntry {
    let schema = schema_for::<CancelOrderInput>();
    let descriptor = Tool::new("cancel_order", "Cancel one open order by its id.", schema);
    let handler: ToolHandlerFn = Arc::new(|ctx, input| Box::pin(handle_cancel_order(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Trading,
        handler,
    }
}

async fn handle_cancel_order(ctx: &AdapterContext, input: Value) -> Result<Value, AdapterError> {
    let input: CancelOrderInput = parse(input)?;
    let order_id = input.order_id.trim();
    if order_id.is_empty() {
        return Err(AdapterError::Validation {
            field: "order_id".to_string(),
            message: "must be non-empty".to_string(),
        });
    }
    let result = ctx.http.cancel_order(order_id).await?;
    Ok(serde_json::to_value(&result)?)
}

// ----- cancel_all_by_currency ---------------------------------------

/// `cancel_all_by_currency` input.
///
/// Mass-cancels every open order in the given currency.
/// Irreversible — use with care.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CancelAllByCurrencyInput {
    /// Currency to mass-cancel orders for (`BTC`, `ETH`, `USDC`, …).
    pub currency: String,
}

fn cancel_all_by_currency_tool() -> ToolEntry {
    let schema = schema_for::<CancelAllByCurrencyInput>();
    let descriptor = Tool::new(
        "cancel_all_by_currency",
        "Mass-cancel every open order for a currency. Returns \
         {\"cancelled\": <count>}. Irreversible — use with care.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_cancel_all_by_currency(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Trading,
        handler,
    }
}

async fn handle_cancel_all_by_currency(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: CancelAllByCurrencyInput = parse(input)?;
    let currency = input.currency.trim();
    if currency.is_empty() {
        return Err(AdapterError::Validation {
            field: "currency".to_string(),
            message: "must be non-empty".to_string(),
        });
    }
    let cancelled = ctx.http.cancel_all_by_currency(currency).await?;
    Ok(serde_json::json!({ "cancelled": cancelled }))
}

// ----- cancel_all_by_instrument -------------------------------------

/// `cancel_all_by_instrument` input.
///
/// Mass-cancels every open order on the given instrument.
/// Irreversible — use with care.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CancelAllByInstrumentInput {
    /// Instrument identifier (`BTC-PERPETUAL`, …).
    pub instrument_name: String,
}

fn cancel_all_by_instrument_tool() -> ToolEntry {
    let schema = schema_for::<CancelAllByInstrumentInput>();
    let descriptor = Tool::new(
        "cancel_all_by_instrument",
        "Mass-cancel every open order on an instrument. Returns \
         {\"cancelled\": <count>}. Irreversible — use with care.",
        schema,
    );
    let handler: ToolHandlerFn =
        Arc::new(|ctx, input| Box::pin(handle_cancel_all_by_instrument(ctx, input)));
    ToolEntry {
        descriptor,
        class: ToolClass::Trading,
        handler,
    }
}

async fn handle_cancel_all_by_instrument(
    ctx: &AdapterContext,
    input: Value,
) -> Result<Value, AdapterError> {
    let input: CancelAllByInstrumentInput = parse(input)?;
    let instrument = input.instrument_name.trim();
    if instrument.is_empty() {
        return Err(AdapterError::Validation {
            field: "instrument_name".to_string(),
            message: "must be non-empty".to_string(),
        });
    }
    let cancelled = ctx.http.cancel_all_by_instrument(instrument).await?;
    Ok(serde_json::json!({ "cancelled": cancelled }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit_input() -> PlaceOrderInput {
        PlaceOrderInput {
            instrument_name: "BTC-PERPETUAL".to_string(),
            side: Side::Buy,
            amount: 10.0,
            order_type: PlaceOrderType::Limit,
            price: Some(50_000.0),
            time_in_force: None,
            label: None,
            trigger_price: None,
            trigger: None,
            post_only: None,
            reject_post_only: None,
            reduce_only: None,
            mmp: None,
            valid_until: None,
        }
    }

    #[test]
    fn limit_without_price_rejected() {
        let mut input = limit_input();
        input.price = None;
        let err = validate_place_order(&input).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "price"));
    }

    #[test]
    fn stop_limit_without_trigger_price_rejected() {
        let mut input = limit_input();
        input.order_type = PlaceOrderType::StopLimit;
        input.trigger = Some(PlaceTrigger::IndexPrice);
        let err = validate_place_order(&input).unwrap_err();
        assert!(
            matches!(err, AdapterError::Validation { ref field, .. } if field == "trigger_price")
        );
    }

    #[test]
    fn stop_limit_without_trigger_rejected() {
        let mut input = limit_input();
        input.order_type = PlaceOrderType::StopLimit;
        input.trigger_price = Some(45_000.0);
        let err = validate_place_order(&input).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "trigger"));
    }

    #[test]
    fn negative_amount_rejected() {
        let mut input = limit_input();
        input.amount = -1.0;
        let err = validate_place_order(&input).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "amount"));
    }

    #[test]
    fn nan_amount_rejected() {
        let mut input = limit_input();
        input.amount = f64::NAN;
        let err = validate_place_order(&input).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "amount"));
    }

    #[test]
    fn nan_price_rejected() {
        let mut input = limit_input();
        input.price = Some(f64::NAN);
        let err = validate_place_order(&input).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "price"));
    }

    #[test]
    fn market_without_price_accepted() {
        let mut input = limit_input();
        input.order_type = PlaceOrderType::Market;
        input.price = None;
        validate_place_order(&input).unwrap();
    }

    #[test]
    fn full_stop_limit_accepted() {
        let mut input = limit_input();
        input.order_type = PlaceOrderType::StopLimit;
        input.trigger_price = Some(45_000.0);
        input.trigger = Some(PlaceTrigger::MarkPrice);
        validate_place_order(&input).unwrap();
    }

    #[test]
    fn build_request_round_trips_buy_side() {
        let input = limit_input();
        let (side, req) = build_order_request(input).unwrap();
        assert_eq!(side, Side::Buy);
        assert_eq!(req.instrument_name, "BTC-PERPETUAL");
        assert_eq!(req.amount, Some(10.0));
        assert_eq!(req.price, Some(50_000.0));
    }

    #[test]
    fn valid_until_overflow_rejected() {
        let mut input = limit_input();
        input.valid_until = Some(u64::MAX);
        let err = build_order_request(input).unwrap_err();
        assert!(
            matches!(err, AdapterError::Validation { ref field, .. } if field == "valid_until")
        );
    }

    fn edit_input() -> EditOrderInput {
        EditOrderInput {
            order_id: "ORDER-1".to_string(),
            amount: Some(20.0),
            price: Some(51_000.0),
            post_only: None,
            reject_post_only: None,
            reduce_only: None,
            mmp: None,
            valid_until: None,
            trigger_price: None,
        }
    }

    #[test]
    fn edit_empty_order_id_rejected() {
        let mut input = edit_input();
        input.order_id = "  ".to_string();
        let err = validate_edit_order(&input).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "order_id"));
    }

    #[test]
    fn edit_negative_amount_rejected() {
        let mut input = edit_input();
        input.amount = Some(-1.0);
        let err = validate_edit_order(&input).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "amount"));
    }

    #[test]
    fn edit_nan_price_rejected() {
        let mut input = edit_input();
        input.price = Some(f64::NAN);
        let err = validate_edit_order(&input).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "price"));
    }

    #[test]
    fn edit_partial_input_accepted() {
        let mut input = edit_input();
        input.price = None;
        validate_edit_order(&input).unwrap();
        let req = build_edit_request(input).unwrap();
        assert_eq!(req.order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(req.amount, Some(20.0));
        assert_eq!(req.price, None);
    }

    #[test]
    fn linear_instrument_classification() {
        assert!(is_linear_instrument("BTC_USDC-PERPETUAL"));
        assert!(is_linear_instrument("ETH_USDT-29SEP24"));
        assert!(!is_linear_instrument("BTC-PERPETUAL"));
        assert!(!is_linear_instrument("ETH-29SEP24"));
        assert!(!is_linear_instrument("BTC-29SEP24-50000-C"));
    }

    #[test]
    fn option_instrument_classification() {
        assert!(is_option_instrument("BTC-29SEP24-50000-C"));
        assert!(is_option_instrument("ETH-30JUN23-1500-P"));
        assert!(!is_option_instrument("BTC-PERPETUAL"));
        assert!(!is_option_instrument("BTC-29SEP24"));
        // Last segment looks like an option flag but the strike
        // doesn't parse — fall back to non-option.
        assert!(!is_option_instrument("BTC-FOO-BAR-C"));
        assert_eq!(option_underlying_base("BTC-29SEP24-50000-C"), Some("BTC"));
        assert_eq!(option_underlying_base("ETH-30JUN23-1500-P"), Some("ETH"));
    }

    fn ctx_with_cap(cap: Option<u64>) -> AdapterContext {
        use crate::config::{Config, LogFormat, Transport};
        use std::net::SocketAddr;
        let cfg = Config {
            endpoint: "https://test.deribit.com".to_string(),
            client_id: Some("id".to_string()),
            client_secret: Some("secret".to_string()),
            allow_trading: true,
            max_order_usd: cap,
            transport: Transport::Stdio,
            http_listen: SocketAddr::from(([127, 0, 0, 1], 8723)),
            http_bearer_token: None,
            log_format: LogFormat::Text,
        };
        AdapterContext::new(Arc::new(cfg)).expect("ctx")
    }

    #[tokio::test]
    async fn cap_unset_is_noop() {
        let ctx = ctx_with_cap(None);
        enforce_size_cap(&ctx, "BTC-PERPETUAL", 1_000_000.0, Some(50_000.0))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn inverse_notional_uses_amount() {
        // BTC-PERPETUAL inverse: amount IS USD notional. 100k > 10k cap.
        let ctx = ctx_with_cap(Some(10_000));
        let err = enforce_size_cap(&ctx, "BTC-PERPETUAL", 100_000.0, Some(50_000.0))
            .await
            .unwrap_err();
        match err {
            AdapterError::SizeCapExceeded { requested, cap } => {
                assert!((requested - 100_000.0).abs() < 1e-6);
                assert!((cap - 10_000.0).abs() < 1e-6);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn inverse_under_cap_passes() {
        let ctx = ctx_with_cap(Some(10_000));
        // 5_000 USD notional, well under the 10_000 cap.
        enforce_size_cap(&ctx, "BTC-PERPETUAL", 5_000.0, Some(50_000.0))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn linear_notional_uses_price_times_amount() {
        // Linear: notional = amount * price = 0.5 * 100_000 = 50_000 > 10_000.
        let ctx = ctx_with_cap(Some(10_000));
        let err = enforce_size_cap(&ctx, "BTC_USDC-PERPETUAL", 0.5, Some(100_000.0))
            .await
            .unwrap_err();
        assert!(matches!(err, AdapterError::SizeCapExceeded { .. }));
    }

    #[test]
    fn edit_no_fields_rejected() {
        let input = EditOrderInput {
            order_id: "ORDER-1".to_string(),
            amount: None,
            price: None,
            post_only: None,
            reject_post_only: None,
            reduce_only: None,
            mmp: None,
            valid_until: None,
            trigger_price: None,
        };
        let err = validate_edit_order(&input).unwrap_err();
        assert!(matches!(err, AdapterError::Validation { ref field, .. } if field == "arguments"));
    }

    #[test]
    fn edit_valid_until_overflow_rejected() {
        let mut input = edit_input();
        input.valid_until = Some(u64::MAX);
        let err = build_edit_request(input).unwrap_err();
        assert!(
            matches!(err, AdapterError::Validation { ref field, .. } if field == "valid_until")
        );
    }
}
