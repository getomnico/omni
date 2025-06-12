use thiserror::Error;

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("Connection error: {0}")]
    Connection(#[from] sqlx::Error),

    #[error("Entity not found")]
    NotFound,

    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Transaction failed: {0}")]
    TransactionFailed(String),
}
