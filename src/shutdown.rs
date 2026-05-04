//! Graceful shutdown helpers.
//!
//! Awaits SIGTERM or Ctrl-C, then resolves. The Axum `with_graceful_shutdown`
//! call drains in-flight requests up to a deadline managed by the caller.

use std::time::Duration;

use tokio::signal;

/// Maximum time the server will spend draining in-flight requests.
pub const DRAIN_DEADLINE: Duration = Duration::from_secs(25);

/// Future that completes on SIGINT (Ctrl-C) or SIGTERM (kubernetes / fly).
pub async fn wait_for_signal() {
    let ctrl_c = async {
        if let Err(err) = signal::ctrl_c().await {
            tracing::error!(error = %err, "failed to install Ctrl-C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
