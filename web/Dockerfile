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
ARG OMNI_VERSION=dev
ARG OMNI_COMMIT=unknown

ENV PUBLIC_OMNI_VERSION=$OMNI_VERSION
ENV PUBLIC_OMNI_COMMIT=$OMNI_COMMIT

# Copy package files for production dependencies
COPY package*.json ./

# Install only production dependencies
RUN npm ci --only=production && npm cache clean --force

# Copy built application from builder stage
COPY --from=builder /app/build ./build
COPY --from=builder /app/static ./static

# Start the application
CMD ["node", "build"]
