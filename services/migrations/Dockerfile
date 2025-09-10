FROM rust:1.85-slim-bookworm

# Install SQLx CLI with postgres features
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install sqlx-cli --no-default-features --features native-tls,postgres

# Create working directory
RUN mkdir /migrations

# Copy migration files
COPY services/migrations/*.sql ./migrations

# Construct DATABASE_URL from environment variables and run migrations
CMD ["bash", "-c", "export DATABASE_URL=\"postgresql://${DATABASE_USERNAME}:${DATABASE_PASSWORD}@${DATABASE_HOST}:${DATABASE_PORT}/${DATABASE_NAME}\" && sqlx migrate run"]
