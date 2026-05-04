//! 501 stub handlers for Chunk-1 Connect-API routes.

use crate::auth::AuthenticatedUser;
use crate::error::AppError;

/// `GET /api/v1/subscription/plans` — auth-optional stub.
pub async fn subscription_plans() -> Result<(), AppError> {
    Err(AppError::not_implemented(
        "Mizan Connect plans & billing is coming in a future release.",
    ))
}

/// `GET /api/v1/sync/brokerage/connections` — authed stub.
pub async fn brokerage_connections(_user: AuthenticatedUser) -> Result<(), AppError> {
    Err(AppError::not_implemented(
        "Mizan Connect brokerage sync is coming in a future release.",
    ))
}

/// `GET /api/v1/sync/brokerage/accounts` — authed stub.
pub async fn brokerage_accounts(_user: AuthenticatedUser) -> Result<(), AppError> {
    Err(AppError::not_implemented(
        "Mizan Connect brokerage sync is coming in a future release.",
    ))
}

/// `GET /api/v1/sync/brokerage/accounts/:id/activities` — authed stub.
pub async fn brokerage_account_activities(_user: AuthenticatedUser) -> Result<(), AppError> {
    Err(AppError::not_implemented(
        "Mizan Connect brokerage sync is coming in a future release.",
    ))
}

/// `GET /api/v1/sync/brokerage/accounts/:id/holdings` — authed stub.
pub async fn brokerage_account_holdings(_user: AuthenticatedUser) -> Result<(), AppError> {
    Err(AppError::not_implemented(
        "Mizan Connect brokerage sync is coming in a future release.",
    ))
}
