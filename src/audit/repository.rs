//! Audit log writer.

use std::net::IpAddr;

use ipnetwork::IpNetwork;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Security-relevant event written to `audit_log`.
#[derive(Debug, Clone)]
pub struct AuditEvent<'a> {
    pub user_id: Option<Uuid>,
    pub event_type: &'a str,
    pub event_data: Option<serde_json::Value>,
    pub ip_address: Option<IpAddr>,
    pub user_agent: Option<&'a str>,
}

impl<'a> AuditEvent<'a> {
    pub fn new(event_type: &'a str) -> Self {
        Self {
            user_id: None,
            event_type,
            event_data: None,
            ip_address: None,
            user_agent: None,
        }
    }

    pub fn user(mut self, id: Uuid) -> Self {
        self.user_id = Some(id);
        self
    }

    pub fn data<T: Serialize>(mut self, value: &T) -> Self {
        self.event_data = serde_json::to_value(value).ok();
        self
    }

    pub fn ip(mut self, ip: IpAddr) -> Self {
        self.ip_address = Some(ip);
        self
    }

    pub fn user_agent(mut self, ua: &'a str) -> Self {
        self.user_agent = Some(ua);
        self
    }
}

/// Insert a single audit log row. Errors are logged at warn level by the
/// caller — audit-write failures must never break the user-facing path.
pub async fn record_event(pool: &PgPool, event: AuditEvent<'_>) -> Result<i64, sqlx::Error> {
    let ip_network: Option<IpNetwork> = event.ip_address.map(IpNetwork::from);
    let id = sqlx::query_scalar!(
        r#"
        INSERT INTO audit_log (user_id, event_type, event_data, ip_address, user_agent)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
        event.user_id,
        event.event_type,
        event.event_data,
        ip_network as Option<IpNetwork>,
        event.user_agent,
    )
    .fetch_one(pool)
    .await?;
    Ok(id)
}
