//! Database wiring (Postgres via SQLx).

pub mod pool;

pub use pool::{connect, run_migrations};
