use crate::config::DatabaseConfig;
use crate::db::error::DatabaseError;
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres};
use std::time::Duration;

#[derive(Clone)]
pub struct DatabasePool {
    pool: PgPool,
    database_url: String,
}

/// A connection acquired from the pool with `app.current_user_id` set.
/// When dropped, the connection returns to the pool.
pub struct UserConn<'a> {
    _guard: sqlx::pool::PoolConnection<Postgres>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> std::ops::Deref for UserConn<'a> {
    type Target = sqlx::PgConnection;
    fn deref(&self) -> &Self::Target {
        &self._guard
    }
}

impl<'a> std::ops::DerefMut for UserConn<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self._guard
    }
}

/// A connection acquired from the pool with `app.is_admin = 'true'` set.
/// Used by indexer/admin operations that need to bypass RLS.
pub struct AdminConn<'a> {
    _guard: sqlx::pool::PoolConnection<Postgres>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> std::ops::Deref for AdminConn<'a> {
    type Target = sqlx::PgConnection;
    fn deref(&self) -> &Self::Target {
        &self._guard
    }
}

impl<'a> std::ops::DerefMut for AdminConn<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self._guard
    }
}

impl DatabasePool {
    pub async fn new(database_url: &str) -> Result<Self, DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(3))
            .connect(database_url)
            .await?;

        Ok(Self {
            pool,
            database_url: database_url.to_string(),
        })
    }

    pub async fn new_with_options(
        database_url: &str,
        max_connections: u32,
        timeout_seconds: u64,
    ) -> Result<Self, DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(timeout_seconds))
            .connect(database_url)
            .await?;

        Ok(Self {
            pool,
            database_url: database_url.to_string(),
        })
    }

    pub async fn from_config(config: &DatabaseConfig) -> Result<Self, DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(Duration::from_secs(config.acquire_timeout_seconds))
            .connect(&config.database_url)
            .await?;

        Ok(Self {
            pool,
            database_url: config.database_url.clone(),
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Acquires a connection from the pool and sets `app.current_user_id`.
    /// Returns a wrapper that implements `Deref`/`DerefMut` to `PgConnection`.
    /// The connection returns to the pool when the wrapper is dropped.
    pub async fn acquire_user(&self, user_id: &str) -> Result<UserConn<'_>, DatabaseError> {
        let mut guard = self.pool.acquire().await?;
        sqlx::query("SET app.current_user_id = $1")
            .bind(user_id)
            .execute(&mut *guard)
            .await?;
        Ok(UserConn {
            _guard: guard,
            _marker: std::marker::PhantomData,
        })
    }

    /// Acquires a connection from the pool and sets `app.is_admin = 'true'`.
    /// Returns a wrapper that implements `Deref`/`DerefMut` to `PgConnection`.
    /// The connection returns to the pool when the wrapper is dropped.
    ///
    /// Used by indexer/admin operations that need to bypass RLS policies.
    pub async fn acquire_admin(&self) -> Result<AdminConn<'_>, DatabaseError> {
        let mut guard = self.pool.acquire().await?;
        sqlx::query("SET app.is_admin = 'true'")
            .execute(&mut *guard)
            .await?;
        Ok(AdminConn {
            _guard: guard,
            _marker: std::marker::PhantomData,
        })
    }
}
