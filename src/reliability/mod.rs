//! Reliability and failover subsystem for Phase 9.
//!
//! Provides bounded retry policies, jittered exponential backoff, transient error
//! classification, and model-compatible failover mechanisms.

pub mod policy;

pub use policy::{RetryPolicy, calculate_backoff, is_non_retryable_error, is_transient_error};
