# Frontend build for Omni SvelteKit application
FROM node:18-alpine AS builder

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
FROM node:18-alpine AS runtime

WORKDIR /app

# Port argument with default value
ARG WEB_PORT=3000

# Copy package files for production dependencies
COPY package*.json ./

# Install only production dependencies
RUN npm ci --only=production && npm cache clean --force

# Copy built application from builder stage
COPY --from=builder /app/build ./build
COPY --from=builder /app/static ./static

# Expose port (SvelteKit production server)
EXPOSE ${WEB_PORT}

# Start the application
CMD ["node", "build"]
