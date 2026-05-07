//! Configuration surface — placeholder for v0.1-02.
//!
//! The configuration loader (CLI + env + `.env`) lands in v0.1-02.
//! This module exists so the rest of the skeleton compiles against
//! `crate::config`.

/// Placeholder marker for the future `Config` struct.
///
/// Replaced in v0.1-02 by the real CLI / env loader. Kept here so the
/// scaffold builds without the implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConfigStub;
