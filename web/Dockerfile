# Frontend build for Omni SvelteKit application
FROM node:24-alpine AS builder

WORKDIR /app

# Copy package files
COPY package*.json ./

# Install dependencies
RUN npm ci

# Copy source code
COPY . .

# Build the application
RUN npm run build

# Production stage
FROM node:24-alpine AS runtime

WORKDIR /app

# Port argument with default value
ARG WEB_PORT=3000

# Detect production adapter-node mode for shutdown sequencing
# (dev docker-compose overrides this to "development")
ENV NODE_ENV=production

# Copy package files for production dependencies
COPY package*.json ./

# Install only production dependencies
RUN npm ci --only=production && npm cache clean --force

# Copy built application from builder stage
COPY --from=builder /app/build ./build
COPY --from=builder /app/static ./static

# Copy the preload dependency set so `node --import ./instrumentation.mjs` resolves
# at startup.  These are plain .mjs files loaded by Node directly (not bundled).
COPY src/instrumentation.mjs ./
COPY src/otel-factory.mjs ./
COPY src/lib/server/otel-utils.mjs ./lib/server/

# Build-stage preload import check: verify the .mjs files exist and parse correctly.
RUN node --check ./instrumentation.mjs && \
    node --check ./otel-factory.mjs && \
    node --check ./lib/server/otel-utils.mjs && \
    echo "Preload dependency syntax check passed."

# Start the application with telemetry preloaded
CMD ["node", "--import", "./instrumentation.mjs", "build"]
