//! Adapter error type that crosses the MCP boundary.
//!
//! `AdapterError` is the only error type the MCP layer ever returns to a
//! client. Upstream errors from `deribit_http`, `deribit_websocket`, and
//! `serde_json` are mapped into one of the structured variants below via
//! `From` impls — the MCP wire never sees raw upstream types.
//!
//! Variants are **structured**, not opaque strings. The serde-tagged
//! representation (`{"kind": "...", ...}`) is what callers parse off the
//! wire when a tool call returns `isError: true`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use deribit_http::HttpError;
use deribit_websocket::error::WebSocketError;

/// Default retry hint when an upstream rate-limit signal does not carry
/// a server-supplied value (e.g. `HttpError::RateLimitExceeded` with no
/// `Retry-After` header).
const DEFAULT_RATE_LIMIT_RETRY_MS: u64 = 1_000;

/// Errors emitted by the adapter and surfaced over the MCP wire.
///
/// Every fallible path in the adapter ultimately produces one of these
/// variants. The serde representation uses an internally tagged
/// `"kind"` discriminant (`#[serde(tag = "kind")]`) so the JSON shape
/// is stable and discoverable by an LLM client.
///
/// Variants are intentionally **closed** — `_` arms on `AdapterError`
/// are forbidden by the project's coding rules. Add a new variant
/// when a structurally new error kind appears.
#[derive(Debug, Error, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
#[non_exhaustive]
pub enum AdapterError {
    /// Authentication / authorization failure.
    #[error("authentication failed: {reason:?}")]
    Auth {
        /// Why authentication failed.
        reason: AuthFailureReason,
    },

    /// Rate-limit signal from upstream. The MCP layer never silently
    /// retries (ADR-0008); it surfaces the hint so the LLM can decide.
    #[error("rate limited; retry after {retry_after_ms} ms")]
    RateLimited {
        /// Suggested wait before re-issuing the call. Zero means
        /// "unknown — back off and retry".
        retry_after_ms: u64,
    },

    /// Upstream returned a structured error that is neither auth nor
    /// rate-limit shaped.
    #[error("upstream error: {inner:?}")]
    Upstream {
        /// Where the error came from and the structured payload.
        #[serde(rename = "source")]
        inner: UpstreamErrorKind,
    },

    /// Input failed validation at the MCP boundary before any upstream
    /// call was made.
    #[error("validation failed for `{field}`: {message}")]
    Validation {
        /// Field name as it appears in the tool's input schema.
        field: String,
        /// Human-readable explanation; safe to surface to an LLM.
        message: String,
    },

    /// `--max-order-usd` cap exceeded. Trading-class concern; surfaced
    /// here so all errors flow through one type.
    #[error("requested {requested} USD exceeds cap {cap} USD")]
    SizeCapExceeded {
        /// Requested notional in USD.
        requested: f64,
        /// Configured cap in USD.
        cap: f64,
    },

    /// A tool exists but is not enabled in this binary's configuration
    /// (e.g. `Trading` without `--allow-trading`).
    #[error("tool `{tool}` requires `{flag}`")]
    NotEnabled {
        /// Tool name as listed in the registry. Construct via
        /// [`AdapterError::not_enabled`] from a `&'static str` literal.
        tool: String,
        /// CLI flag that would enable it.
        flag: String,
    },

    /// Last-resort variant for failures that should not propagate
    /// detail across the MCP boundary (e.g. an upstream payload that
    /// might leak a signature). The original error is logged at DEBUG
    /// with the redaction filter active.
    #[error("internal error: {reason}")]
    Internal {
        /// Pre-vetted reason string. Construct via
        /// [`AdapterError::internal`] from a `&'static str` literal —
        /// never user-controlled content.
        reason: String,
    },
}

/// Why authentication failed. Closed set — exhaustive matches required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuthFailureReason {
    /// Credentials missing — neither client_id nor client_secret were
    /// configured.
    MissingCredentials,
    /// Credentials present but rejected by Deribit.
    Unauthorized,
    /// Token refresh / fork failed mid-session.
    TokenRefreshFailed,
    /// Adapter recognised the upstream auth error but couldn't classify
    /// it any further.
    Other,
}

/// Structured upstream-error payload. Closed set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UpstreamErrorKind {
    /// Deribit JSON-RPC API error: a non-rate-limit, non-auth failure
    /// returned with a `code` + `message` body.
    Api {
        /// Deribit error code, when available.
        code: Option<i64>,
        /// Human-readable message after secret redaction.
        message: String,
    },
    /// Network-layer error (connect, TLS, DNS, …).
    Network {
        /// Short description; safe for the wire.
        message: String,
    },
    /// HTTP transport error that doesn't fit the structured shapes
    /// above (e.g. invalid response, parse).
    Http {
        /// Short description; safe for the wire.
        message: String,
    },
    /// WebSocket transport error.
    Websocket {
        /// Short description; safe for the wire.
        message: String,
    },
}

impl AdapterError {
    /// Convenience constructor for [`AdapterError::Validation`].
    #[cold]
    #[inline(never)]
    #[must_use]
    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            field: field.into(),
            message: message.into(),
        }
    }

    /// Convenience constructor for [`AdapterError::RateLimited`] from a
    /// [`Duration`].
    #[cold]
    #[inline(never)]
    #[must_use]
    pub fn rate_limited(retry_after: Duration) -> Self {
        let retry_after_ms = u64::try_from(retry_after.as_millis()).unwrap_or(u64::MAX);
        Self::RateLimited { retry_after_ms }
    }
}

impl From<HttpError> for AdapterError {
    fn from(err: HttpError) -> Self {
        match err {
            HttpError::AuthenticationFailed(_) => AdapterError::Auth {
                reason: AuthFailureReason::Unauthorized,
            },
            HttpError::RateLimitExceeded => AdapterError::RateLimited {
                retry_after_ms: DEFAULT_RATE_LIMIT_RETRY_MS,
            },
            HttpError::NetworkError(message) => AdapterError::Upstream {
                inner: UpstreamErrorKind::Network { message },
            },
            HttpError::RequestFailed(message)
            | HttpError::InvalidResponse(message)
            | HttpError::ParseError(message) => AdapterError::Upstream {
                inner: UpstreamErrorKind::Http { message },
            },
            HttpError::ConfigError(_) => {
                AdapterError::internal("upstream HTTP client misconfigured")
            }
        }
    }
}

impl From<WebSocketError> for AdapterError {
    fn from(err: WebSocketError) -> Self {
        match err {
            WebSocketError::AuthenticationFailed(_) => AdapterError::Auth {
                reason: AuthFailureReason::Unauthorized,
            },
            WebSocketError::ApiError { code, message, .. } => match code {
                10009 | 10028 | 10040 | 10041 => AdapterError::RateLimited {
                    retry_after_ms: DEFAULT_RATE_LIMIT_RETRY_MS,
                },
                10000 | 10001 | 10002 | 13004 | 13005 | 13007 | 13008 | 13009 => {
                    AdapterError::Auth {
                        reason: AuthFailureReason::Unauthorized,
                    }
                }
                _ => AdapterError::Upstream {
                    inner: UpstreamErrorKind::Api {
                        code: Some(code),
                        message,
                    },
                },
            },
            // Every other WS variant flows through the generic
            // `Websocket` upstream payload so the LLM still sees a
            // structured error rather than `Internal`. The string is
            // truncated to keep the wire payload bounded.
            other => AdapterError::Upstream {
                inner: UpstreamErrorKind::Websocket {
                    message: ws_short(&other.to_string()),
                },
            },
        }
    }
}

/// Truncate a one-line WS error string to a stable, bounded wire size.
#[inline]
fn ws_short(s: &str) -> String {
    const MAX: usize = 256;
    if s.len() <= MAX {
        s.to_string()
    } else {
        let mut end = MAX;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        let mut out = String::with_capacity(end + 1);
        out.push_str(&s[..end]);
        out.push('…');
        out
    }
}

impl From<serde_json::Error> for AdapterError {
    fn from(_err: serde_json::Error) -> Self {
        AdapterError::internal("upstream payload schema mismatch")
    }
}

impl AdapterError {
    /// Convenience constructor for [`AdapterError::NotEnabled`].
    #[cold]
    #[inline(never)]
    #[must_use]
    pub fn not_enabled(tool: &'static str, flag: &'static str) -> Self {
        Self::NotEnabled {
            tool: tool.to_string(),
            flag: flag.to_string(),
        }
    }

    /// Convenience constructor for [`AdapterError::Internal`].
    #[cold]
    #[inline(never)]
    #[must_use]
    pub fn internal(reason: &'static str) -> Self {
        Self::Internal {
            reason: reason.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(err: &AdapterError) -> AdapterError {
        let json = serde_json::to_string(err).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn auth_round_trip() {
        for reason in [
            AuthFailureReason::MissingCredentials,
            AuthFailureReason::Unauthorized,
            AuthFailureReason::TokenRefreshFailed,
            AuthFailureReason::Other,
        ] {
            let err = AdapterError::Auth { reason };
            assert_eq!(err, round_trip(&err));
        }
    }

    #[test]
    fn rate_limited_round_trip() {
        let err = AdapterError::RateLimited {
            retry_after_ms: 2_000,
        };
        assert_eq!(err, round_trip(&err));
    }

    #[test]
    fn upstream_api_round_trip() {
        let err = AdapterError::Upstream {
            inner: UpstreamErrorKind::Api {
                code: Some(10000),
                message: "boom".to_string(),
            },
        };
        assert_eq!(err, round_trip(&err));
    }

    #[test]
    fn upstream_network_round_trip() {
        let err = AdapterError::Upstream {
            inner: UpstreamErrorKind::Network {
                message: "dns".to_string(),
            },
        };
        assert_eq!(err, round_trip(&err));
    }

    #[test]
    fn upstream_websocket_round_trip() {
        let err = AdapterError::Upstream {
            inner: UpstreamErrorKind::Websocket {
                message: "closed".to_string(),
            },
        };
        assert_eq!(err, round_trip(&err));
    }

    #[test]
    fn validation_round_trip() {
        let err = AdapterError::validation("instrument_name", "must be non-empty");
        assert_eq!(err, round_trip(&err));
    }

    #[test]
    fn size_cap_exceeded_round_trip() {
        let err = AdapterError::SizeCapExceeded {
            requested: 25_000.0,
            cap: 10_000.0,
        };
        assert_eq!(err, round_trip(&err));
    }

    #[test]
    fn not_enabled_round_trip() {
        let err = AdapterError::not_enabled("place_order", "--allow-trading");
        assert_eq!(err, round_trip(&err));
    }

    #[test]
    fn internal_round_trip() {
        let err = AdapterError::internal("upstream payload schema mismatch");
        assert_eq!(err, round_trip(&err));
    }

    #[test]
    fn http_authentication_failed_maps_to_auth_unauthorized() {
        let err: AdapterError = HttpError::AuthenticationFailed("bad creds".into()).into();
        assert_eq!(
            err,
            AdapterError::Auth {
                reason: AuthFailureReason::Unauthorized
            }
        );
    }

    #[test]
    fn http_rate_limit_exceeded_maps_to_rate_limited() {
        let err: AdapterError = HttpError::RateLimitExceeded.into();
        assert!(matches!(err, AdapterError::RateLimited { .. }));
    }

    #[test]
    fn http_network_error_maps_to_upstream_network() {
        let err: AdapterError = HttpError::NetworkError("connect".into()).into();
        assert!(matches!(
            err,
            AdapterError::Upstream {
                inner: UpstreamErrorKind::Network { .. }
            }
        ));
    }

    #[test]
    fn http_request_failed_maps_to_upstream_http() {
        let err: AdapterError = HttpError::RequestFailed("500".into()).into();
        assert!(matches!(
            err,
            AdapterError::Upstream {
                inner: UpstreamErrorKind::Http { .. }
            }
        ));
    }

    #[test]
    fn http_config_error_maps_to_internal() {
        let err: AdapterError = HttpError::ConfigError("bad url".into()).into();
        assert!(matches!(err, AdapterError::Internal { .. }));
    }

    #[test]
    fn ws_authentication_failed_maps_to_auth_unauthorized() {
        let err: AdapterError = WebSocketError::AuthenticationFailed("bad".into()).into();
        assert_eq!(
            err,
            AdapterError::Auth {
                reason: AuthFailureReason::Unauthorized
            }
        );
    }

    #[test]
    fn ws_api_error_rate_limit_code_maps_to_rate_limited() {
        let err: AdapterError = WebSocketError::ApiError {
            code: 10028,
            message: "too many".into(),
            method: None,
            params: None,
            raw_response: None,
        }
        .into();
        assert!(matches!(err, AdapterError::RateLimited { .. }));
    }

    #[test]
    fn ws_api_error_other_code_maps_to_upstream_api() {
        let err: AdapterError = WebSocketError::ApiError {
            code: 11099,
            message: "boom".into(),
            method: None,
            params: None,
            raw_response: None,
        }
        .into();
        match err {
            AdapterError::Upstream {
                inner: UpstreamErrorKind::Api { code, .. },
            } => {
                assert_eq!(code, Some(11099));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn serde_json_error_maps_to_internal() {
        let parse: Result<i32, _> = serde_json::from_str("not json");
        let err: AdapterError = parse.unwrap_err().into();
        assert!(matches!(err, AdapterError::Internal { .. }));
    }
}
