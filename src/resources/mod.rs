//! Resource registry and `deribit://` URI parsing.
//!
//! Resources are partitioned by lifetime:
//!
//! - [`static_`] — refresh-on-read resources backed by `deribit-http`.
//! - [`live`] — WebSocket-backed subscribable resources (lands in v0.3
//!   per ADR-0006).

pub mod live;
pub mod static_;
