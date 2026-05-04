//! Append-only security/admin audit trail.
//!
//! Never log financial data here — that lives in domain tables. This is
//! purely for security-relevant events: logins, key rotation, admin
//! overrides, etc. Future chunks will populate this from auth + billing
//! handlers.

pub mod repository;

pub use repository::{record_event, AuditEvent};
