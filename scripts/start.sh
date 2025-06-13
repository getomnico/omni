#!/bin/bash

# Clio Startup Script
# This script sets up and starts the complete Clio enterprise search platform

set -e

echo "🚀 Starting Clio Enterprise Search Platform"
echo "============================================="

# Check if Docker and Docker Compose are installed
if ! command -v docker &> /dev/null; then
    echo "❌ Docker is not installed. Please install Docker first."
    exit 1
fi

if ! command -v docker-compose &> /dev/null && ! docker compose version &> /dev/null; then
    echo "❌ Docker Compose is not installed. Please install Docker Compose first."
    exit 1
fi

# Check if .env file exists
if [ ! -f .env ]; then
    echo "⚠️  No .env file found. Creating from template..."
    cp .env.example .env
    echo "📝 Please edit .env file with your configuration before running again."
    echo "   Especially important: JWT_SECRET and connector credentials"
    exit 1
fi

# Create necessary directories
echo "📁 Creating necessary directories..."
mkdir -p data/postgres
mkdir -p data/redis
mkdir -p data/models

# Start the services
echo "🐳 Starting Docker containers..."
docker-compose up -d

# Wait for services to be healthy
echo "⏳ Waiting for services to start..."
sleep 10

# Check service health
echo "🔍 Checking service health..."
services=("postgres" "redis" "web" "searcher" "indexer" "ai")

for service in "${services[@]}"; do
    if docker-compose ps | grep -q "$service.*Up"; then
        echo "✅ $service is running"
    else
        echo "❌ $service failed to start"
        docker-compose logs "$service"
    fi
done

echo ""
echo "🎉 Clio is starting up!"
echo "========================"
echo "Frontend:     http://localhost"
echo "API Gateway:  http://localhost:3000"
echo "Dashboard:    http://localhost (when ready)"
echo ""
echo "📊 Service Status:"
echo "   Web Application:     http://localhost:3000"
echo "   Search Service:      http://localhost:3001/health"
echo "   Indexer Service:     http://localhost:3002/health" 
echo "   AI Service:          http://localhost:3003/health"
echo "   Google Connector:    http://localhost:4001/health"
echo "   Slack Connector:     http://localhost:4002/health"
echo "   Atlassian Connector: http://localhost:4003/health"
echo ""
echo "📝 Logs: docker-compose logs -f [service-name]"
echo "🛑 Stop:  docker-compose down"
echo ""
echo "⚠️  Note: First startup may take a few minutes to download images and initialize databases."