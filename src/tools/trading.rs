//! `Trading` tool family — populated in v0.4.
//!
//! All tools in this module have [`ToolClass::Trading`]. The registry
//! refuses to register them unless `--allow-trading` is passed
//! (ADR-0010), and `--max-order-usd` caps notional size.
//!
//! [`ToolClass::Trading`]: super::ToolClass::Trading
