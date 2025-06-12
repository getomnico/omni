use shared::{DatabasePool, Repository, User, UserRepository, UserRole};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Example of how services will use the shared database layer
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/clio".to_string());

    // Initialize database pool
    let db_pool = DatabasePool::new(&database_url).await?;

    // Create repository instances
    let user_repo = UserRepository::new(db_pool.pool());

    // Example: Find user by email
    let user = user_repo.find_by_email("user@example.com").await?;
    println!("Found user: {:?}", user);

    // Example: Find all admins
    let admins = user_repo.find_by_role(UserRole::Admin).await?;
    println!("Found {} admins", admins.len());

    // Example: Create a new user
    let new_user = User {
        id: "01234567890123456789012345".to_string(), // ULID
        email: "newuser@example.com".to_string(),
        password_hash: "hashed_password".to_string(),
        full_name: Some("New User".to_string()),
        avatar_url: None,
        role: UserRole::User,
        is_active: true,
        created_at: sqlx::types::time::OffsetDateTime::now_utc(),
        updated_at: sqlx::types::time::OffsetDateTime::now_utc(),
        last_login_at: None,
    };

    let created_user = user_repo.create(new_user).await?;
    println!("Created user: {:?}", created_user);

    Ok(())
}
