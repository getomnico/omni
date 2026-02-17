#!/bin/bash
set -e

# Omni Latency Benchmark Runner
# Usage: ./run_latency_benchmark.sh [--concurrency N] [--warmup N] [--dataset DATASET] [--search-mode MODE]

# Default values
CONCURRENCY=10
WARMUP=100
DATASET="beir"
SEARCH_MODE="all"
CONFIG="benchmarks/config/default.toml"
START_SERVICES=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --concurrency)
            CONCURRENCY="$2"
            shift 2
            ;;
        --warmup)
            WARMUP="$2"
            shift 2
            ;;
        --dataset)
            DATASET="$2"
            shift 2
            ;;
        --search-mode)
            SEARCH_MODE="$2"
            shift 2
            ;;
        --config)
            CONFIG="$2"
            shift 2
            ;;
        --start-services)
            START_SERVICES=true
            shift
            ;;
        --help)
            echo "Omni Latency Benchmark"
            echo ""
            echo "Usage: $0 [options]"
            echo ""
            echo "Options:"
            echo "  --concurrency N      Concurrent query execution (default: 10)"
            echo "  --warmup N           Warmup queries (default: 100)"
            echo "  --dataset DATASET    Dataset to benchmark: beir, msmarco, nq (default: beir)"
            echo "  --search-mode MODE   Search mode: fulltext, semantic, hybrid, all (default: all)"
            echo "  --config PATH        Config file path (default: benchmarks/config/default.toml)"
            echo "  --start-services     Start Docker services before benchmark"
            echo "  --help               Show this help message"
            echo ""
            echo "Examples:"
            echo "  # Run with defaults (BEIR, all search modes)"
            echo "  $0"
            echo ""
            echo "  # Run with high concurrency on NQ dataset"
            echo "  $0 --concurrency 20 --dataset nq"
            echo ""
            echo "  # Run hybrid-only on MS MARCO"
            echo "  $0 --dataset msmarco --search-mode hybrid"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo "========================================"
echo "Omni Latency Benchmark"
echo "========================================"
echo ""

# Start services if requested
if [ "$START_SERVICES" = true ]; then
    echo "Starting Docker services..."
    docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml -f docker/docker-compose.benchmark.yml --env-file .env up -d
    echo "Waiting for services to be ready..."
    sleep 5
fi

# Check if services are running
echo "Checking service health..."

if ! curl -sf http://localhost:3001/health > /dev/null 2>&1; then
    echo "ERROR: Searcher service not available at http://localhost:3001"
    echo "Please start services with: docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env up -d"
    exit 1
fi
echo "✓ Searcher service is healthy"

if ! curl -sf http://localhost:3002/health > /dev/null 2>&1; then
    echo "WARNING: Indexer service not available at http://localhost:3002"
    echo "Indexing may not work correctly"
else
    echo "✓ Indexer service is healthy"
fi

echo ""
echo "Configuration:"
echo "  Config file: $CONFIG"
echo "  Dataset: $DATASET"
echo "  Search mode: $SEARCH_MODE"
echo "  Concurrency: $CONCURRENCY"
echo "  Warmup: $WARMUP"
echo ""

# Run benchmark
echo "Starting benchmark..."
cargo run --release -p omni-benchmarks -- run \
    --config "$CONFIG" \
    --dataset "$DATASET" \
    --search-mode "$SEARCH_MODE" \
    --warmup "$WARMUP" \
    --concurrency "$CONCURRENCY"

echo ""
echo "========================================"
echo "Benchmark complete!"
echo "========================================"
