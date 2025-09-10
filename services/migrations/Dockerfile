FROM rust:1.85-slim-bookworm

# Install SQLx CLI with postgres features and jq for URL encoding
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    jq \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install sqlx-cli --no-default-features --features native-tls,postgres

# Create working directory
RUN mkdir /migrations

# Copy migration files
COPY services/migrations/*.sql ./migrations

# Construct DATABASE_URL from environment variables and run migrations
# URL-encode the password to handle special characters
CMD ["bash", "-c", "ENCODED_PASSWORD=$(printf '%s' \"$DATABASE_PASSWORD\" | jq -sRr @uri) && export DATABASE_URL=\"postgresql://${DATABASE_USERNAME}:${ENCODED_PASSWORD}@${DATABASE_HOST}:${DATABASE_PORT}/${DATABASE_NAME}\" && sqlx migrate run"]
