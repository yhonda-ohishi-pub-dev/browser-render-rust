use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use thiserror::Error;
use tracing::{error, info};

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Session not found")]
    SessionNotFound,
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// Session data stored in the database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub user_id: String,
    pub company_id: String,
}

/// Cookie data stored in the database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires_at: DateTime<Utc>,
    pub http_only: bool,
    pub secure: bool,
}

/// Vehicle cache entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleCache {
    pub vehicle_cd: String,
    pub data: String,
    pub cached_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// SQLite storage implementation
#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    /// Create a new storage instance
    pub async fn new(db_path: &str) -> Result<Self> {
        // Ensure the data directory exists
        if let Some(parent) = std::path::Path::new(db_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Create database URL
        let db_url = format!("sqlite:{}?mode=rwc", db_path);

        // Create connection pool
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await?;

        let storage = Storage { pool };
        storage.initialize().await?;
        storage.optimize().await?;

        info!("Storage initialized successfully");
        Ok(storage)
    }

    /// Initialize database schema
    async fn initialize(&self) -> Result<()> {
        let queries = [
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                user_id TEXT,
                company_id TEXT
            )
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS cookies (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                name TEXT NOT NULL,
                value TEXT NOT NULL,
                domain TEXT,
                path TEXT,
                expires_at TEXT,
                http_only INTEGER DEFAULT 0,
                secure INTEGER DEFAULT 0,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            )
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS vehicle_cache (
                vehicle_cd TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                cached_at TEXT NOT NULL,
                expires_at TEXT NOT NULL
            )
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS kv_store (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                ttl INTEGER
            )
            "#,
        ];

        for query in queries {
            sqlx::query(query).execute(&self.pool).await?;
        }

        Ok(())
    }

    /// Optimize SQLite settings
    async fn optimize(&self) -> Result<()> {
        let pragmas = [
            "PRAGMA journal_mode=WAL",
            "PRAGMA synchronous=NORMAL",
            "PRAGMA cache_size=10000",
            "PRAGMA temp_store=MEMORY",
        ];

        for pragma in pragmas {
            sqlx::query(pragma).execute(&self.pool).await?;
        }

        Ok(())
    }

    // ========== Session Methods ==========

    /// Create a new session
    pub async fn create_session(&self, session: &Session) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO sessions (id, created_at, updated_at, expires_at, user_id, company_id)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&session.id)
        .bind(session.created_at.to_rfc3339())
        .bind(session.updated_at.to_rfc3339())
        .bind(session.expires_at.to_rfc3339())
        .bind(&session.user_id)
        .bind(&session.company_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get a session by ID (returns None if expired)
    pub async fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
        let now = Utc::now().to_rfc3339();

        let row = sqlx::query(
            r#"
            SELECT id, created_at, updated_at, expires_at, user_id, company_id
            FROM sessions
            WHERE id = ? AND expires_at > ?
            "#,
        )
        .bind(session_id)
        .bind(&now)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let session = Session {
                    id: row.get("id"),
                    created_at: parse_datetime(row.get("created_at")),
                    updated_at: parse_datetime(row.get("updated_at")),
                    expires_at: parse_datetime(row.get("expires_at")),
                    user_id: row.get("user_id"),
                    company_id: row.get("company_id"),
                };
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }

    /// Delete a session and its associated cookies
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Delete cookies first
        sqlx::query("DELETE FROM cookies WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        // Delete session
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    // ========== Cookie Methods ==========

    /// Save cookies for a session (replaces existing)
    pub async fn save_cookies(&self, session_id: &str, cookies: &[Cookie]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Clear existing cookies
        sqlx::query("DELETE FROM cookies WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        // Insert new cookies
        for cookie in cookies {
            sqlx::query(
                r#"
                INSERT INTO cookies (session_id, name, value, domain, path, expires_at, http_only, secure)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(session_id)
            .bind(&cookie.name)
            .bind(&cookie.value)
            .bind(&cookie.domain)
            .bind(&cookie.path)
            .bind(cookie.expires_at.to_rfc3339())
            .bind(cookie.http_only as i32)
            .bind(cookie.secure as i32)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Get all cookies for a session
    pub async fn get_cookies(&self, session_id: &str) -> Result<Vec<Cookie>> {
        let rows = sqlx::query(
            r#"
            SELECT name, value, domain, path, expires_at, http_only, secure
            FROM cookies
            WHERE session_id = ?
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        let cookies = rows
            .into_iter()
            .map(|row| Cookie {
                name: row.get("name"),
                value: row.get("value"),
                domain: row.get("domain"),
                path: row.get("path"),
                expires_at: parse_datetime(row.get("expires_at")),
                http_only: row.get::<i32, _>("http_only") != 0,
                secure: row.get::<i32, _>("secure") != 0,
            })
            .collect();

        Ok(cookies)
    }

    // ========== Vehicle Cache Methods ==========

    /// Cache vehicle data with TTL
    pub async fn cache_vehicle_data<T: Serialize>(
        &self,
        vehicle_cd: &str,
        data: &T,
        ttl: Duration,
    ) -> Result<()> {
        let json_data = serde_json::to_string(data)?;
        let now = Utc::now();
        let expires_at = now + chrono::Duration::from_std(ttl).unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO vehicle_cache (vehicle_cd, data, cached_at, expires_at)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(vehicle_cd) DO UPDATE SET
                data = excluded.data,
                cached_at = excluded.cached_at,
                expires_at = excluded.expires_at
            "#,
        )
        .bind(vehicle_cd)
        .bind(&json_data)
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get cached vehicle data (returns None if expired)
    pub async fn get_cached_vehicle_data(&self, vehicle_cd: &str) -> Result<Option<String>> {
        let now = Utc::now().to_rfc3339();

        let row = sqlx::query(
            r#"
            SELECT data FROM vehicle_cache
            WHERE vehicle_cd = ? AND expires_at > ?
            "#,
        )
        .bind(vehicle_cd)
        .bind(&now)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.get("data")))
    }

    // ========== KV Store Methods ==========

    /// Set a key-value pair
    pub async fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let json_data = serde_json::to_string(value)?;
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO kv_store (key, value, created_at, updated_at)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(key)
        .bind(&json_data)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get a value by key
    pub async fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<Option<T>> {
        let row = sqlx::query("SELECT value FROM kv_store WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(row) => {
                let data: String = row.get("value");
                let value = serde_json::from_str(&data)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// Delete a key-value pair
    pub async fn delete(&self, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM kv_store WHERE key = ?")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ========== Maintenance Methods ==========

    /// Cleanup expired data
    pub async fn cleanup_expired(&self) -> Result<()> {
        let now = Utc::now().to_rfc3339();

        let queries = [
            "DELETE FROM sessions WHERE expires_at < ?",
            "DELETE FROM vehicle_cache WHERE expires_at < ?",
            "DELETE FROM cookies WHERE expires_at < ?",
        ];

        for query in queries {
            if let Err(e) = sqlx::query(query).bind(&now).execute(&self.pool).await {
                error!("Error cleaning up expired data: {}", e);
            }
        }

        Ok(())
    }

    /// Close the storage connection
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Parse datetime string to DateTime<Utc>
fn parse_datetime(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
