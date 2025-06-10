use async_trait::async_trait;
use sqlx::PgPool;
use crate::{
    db::error::DatabaseError,
    models::{User, UserRole},
    traits::Repository,
};

pub struct UserRepository {
    pool: PgPool,
}

impl UserRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self {
            pool: pool.clone(),
        }
    }
    
    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>, DatabaseError> {
        let user = sqlx::query_as::<_, User>(
            r#"
            SELECT id, email, password_hash, full_name, avatar_url,
                   role, is_active, created_at, updated_at, last_login_at
            FROM users
            WHERE email = $1
            "#
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(user)
    }
    
    pub async fn find_by_role(&self, role: UserRole) -> Result<Vec<User>, DatabaseError> {
        let role_str = match role {
            UserRole::Admin => "admin",
            UserRole::User => "user",
            UserRole::Viewer => "viewer",
        };
        
        let users = sqlx::query_as::<_, User>(
            r#"
            SELECT id, email, password_hash, full_name, avatar_url,
                   role, is_active, created_at, updated_at, last_login_at
            FROM users
            WHERE role = $1
            ORDER BY created_at DESC
            "#
        )
        .bind(role_str)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(users)
    }
}

#[async_trait]
impl Repository<User, String> for UserRepository {
    async fn find_by_id(&self, id: String) -> Result<Option<User>, DatabaseError> {
        let user = sqlx::query_as::<_, User>(
            r#"
            SELECT id, email, password_hash, full_name, avatar_url,
                   role, is_active, created_at, updated_at, last_login_at
            FROM users
            WHERE id = $1
            "#
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(user)
    }
    
    async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<User>, DatabaseError> {
        let users = sqlx::query_as::<_, User>(
            r#"
            SELECT id, email, password_hash, full_name, avatar_url,
                   role, is_active, created_at, updated_at, last_login_at
            FROM users
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        
        Ok(users)
    }
    
    async fn create(&self, user: User) -> Result<User, DatabaseError> {
        let role_str = match user.role {
            UserRole::Admin => "admin",
            UserRole::User => "user",
            UserRole::Viewer => "viewer",
        };
        
        let created_user = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (id, email, password_hash, full_name, avatar_url, role, is_active)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, email, password_hash, full_name, avatar_url,
                      role, is_active, created_at, updated_at, last_login_at
            "#
        )
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.password_hash)
        .bind(&user.full_name)
        .bind(&user.avatar_url)
        .bind(role_str)
        .bind(user.is_active)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                DatabaseError::ConstraintViolation("Email already exists".to_string())
            }
            _ => DatabaseError::from(e),
        })?;
        
        Ok(created_user)
    }
    
    async fn update(&self, id: String, user: User) -> Result<Option<User>, DatabaseError> {
        let role_str = match user.role {
            UserRole::Admin => "admin",
            UserRole::User => "user",
            UserRole::Viewer => "viewer",
        };
        
        let updated_user = sqlx::query_as::<_, User>(
            r#"
            UPDATE users
            SET email = $2, password_hash = $3, full_name = $4, avatar_url = $5, role = $6, is_active = $7
            WHERE id = $1
            RETURNING id, email, password_hash, full_name, avatar_url,
                      role, is_active, created_at, updated_at, last_login_at
            "#
        )
        .bind(&id)
        .bind(&user.email)
        .bind(&user.password_hash)
        .bind(&user.full_name)
        .bind(&user.avatar_url)
        .bind(role_str)
        .bind(user.is_active)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(updated_user)
    }
    
    async fn delete(&self, id: String) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(&id)
            .execute(&self.pool)
            .await?;
        
        Ok(result.rows_affected() > 0)
    }
}