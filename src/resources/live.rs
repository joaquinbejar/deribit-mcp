//! Live (WebSocket-backed, subscribable) resources — populated in v0.3.
//!
//! Subscriptions are fanned out from one upstream `deribit-websocket`
//! channel reader to N MCP subscribers; reference-counted teardown.
