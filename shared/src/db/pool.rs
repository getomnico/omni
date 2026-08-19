use crate::config::DatabaseConfig;
use crate::db::error::DatabaseError;
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use std::{env, time::Duration};
use url::Url;

pub async fn begin_document_user_on<'a>(
    pool: &'a PgPool,
    email: &str,
    public_only: bool,
) -> Result<Transaction<'a, Postgres>, DatabaseError> {
    let mut tx = pool.begin().await?;
    // No-op in production (the pool already logs in as omni_user); switches the
    // owner connection to the user role in tests and maintenance sessions.
    sqlx::query("SET LOCAL ROLE omni_user")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('omni.document_user_email', $1, true)")
        .bind(email)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('omni.document_access_scope', $1, true)")
        .bind(if public_only { "public" } else { "user" })
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

pub async fn begin_document_system_on<'a>(
    pool: &'a PgPool,
) -> Result<Transaction<'a, Postgres>, DatabaseError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL ROLE omni_system")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

#[derive(Clone)]
pub struct DatabasePool {
    pool: PgPool,
    system_pool: Option<PgPool>,
    database_url: String,
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
            system_pool: None,
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
            system_pool: None,
            database_url: database_url.to_string(),
        })
    }

    pub async fn from_config(config: &DatabaseConfig) -> Result<Self, DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(Duration::from_secs(config.acquire_timeout_seconds))
            .connect(&config.database_url)
            .await?;
        let system_pool = Self::connect_system_pool(config).await?;

        Ok(Self {
            pool,
            system_pool,
            database_url: config.database_url.clone(),
        })
    }

    async fn connect_system_pool(config: &DatabaseConfig) -> Result<Option<PgPool>, DatabaseError> {
        let credentials = (
            env::var("DATABASE_SYSTEM_USERNAME").ok(),
            env::var("DATABASE_SYSTEM_PASSWORD").ok(),
        );
        let (username, password) = match credentials {
            (Some(username), Some(password)) => (username, password),
            (None, None) => return Ok(None),
            _ => {
                return Err(DatabaseError::InvalidInput(
                    "DATABASE_SYSTEM_USERNAME and DATABASE_SYSTEM_PASSWORD must be set together"
                        .to_string(),
                ));
            }
        };

        let mut url = Url::parse(&config.database_url).map_err(|error| {
            DatabaseError::InvalidInput(format!("Invalid system database URL: {error}"))
        })?;
        url.set_username(&username).map_err(|_| {
            DatabaseError::InvalidInput("Invalid system database username".to_string())
        })?;
        url.set_password(Some(&password)).map_err(|_| {
            DatabaseError::InvalidInput("Invalid system database password".to_string())
        })?;

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(Duration::from_secs(config.acquire_timeout_seconds))
            .connect(url.as_str())
            .await?;
        Ok(Some(pool))
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn system_pool(&self) -> &PgPool {
        self.system_pool.as_ref().unwrap_or(&self.pool)
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub async fn close(&self) {
        self.pool.close().await;
        if let Some(pool) = &self.system_pool {
            pool.close().await;
        }
    }

    pub async fn begin_document_user(
        &self,
        email: &str,
        public_only: bool,
    ) -> Result<Transaction<'_, Postgres>, DatabaseError> {
        begin_document_user_on(&self.pool, email, public_only).await
    }

    pub async fn begin_document_system(&self) -> Result<Transaction<'_, Postgres>, DatabaseError> {
        begin_document_system_on(self.system_pool()).await
    }
}
