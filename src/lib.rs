//! Mizan Connect — proprietary backend for the Mizan desktop client.
//!
//! Public surface for binary and integration tests. Composition is in
//! [`server::build_app`]; everything else is wiring.

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

pub mod audit;
pub mod auth;
pub mod billing;
pub mod config;
pub mod connect;
pub mod db;
pub mod error;
pub mod health;
pub mod middleware;
pub mod server;
pub mod shutdown;
pub mod snaptrade;
pub mod state;
pub mod telemetry;
pub mod users;

pub use config::Config;
pub use error::{AppError, ErrorCode};
pub use state::AppState;

/// Application version string, baked in at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Short git commit hash, baked in at build time. `unknown` outside a checkout.
pub const GIT_COMMIT: &str = env!("MIZAN_GIT_COMMIT");

/// RFC3339 build timestamp, baked in at build time.
pub const BUILD_TIME: &str = env!("MIZAN_BUILD_TIME");
