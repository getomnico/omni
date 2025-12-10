#!/bin/bash
set -e

# ParadeDB Latency Benchmark Runner
# Usage: ./run_latency_benchmark.sh [--num-queries N] [--target-qps N] [--max-docs N] [--skip-indexing] [--cleanup]

# Default values
NUM_QUERIES=1000
CONCURRENCY=10
WARMUP=100
TARGET_QPS=0  # 0 = unlimited burst mode
MAX_DOCS=""
SKIP_INDEXING=""
CLEANUP=""
DATA_DIR="benchmarks/data/nq_benchmark"
CONFIG="benchmarks/config/latency.toml"
USE_BENCHMARK_DB=true
START_SERVICES=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --num-queries)
            NUM_QUERIES="$2"
            shift 2
            ;;
        --concurrency)
            CONCURRENCY="$2"
            shift 2
            ;;
        --warmup)
            WARMUP="$2"
            shift 2
            ;;
        --target-qps)
            TARGET_QPS="$2"
            shift 2
            ;;
        --max-docs)
            MAX_DOCS="--max-documents $2"
            shift 2
            ;;
        --skip-indexing)
            SKIP_INDEXING="--skip-indexing"
            shift
            ;;
        --cleanup)
            CLEANUP="--cleanup"
            shift
            ;;
        --data-dir)
            DATA_DIR="$2"
            shift 2
            ;;
        --config)
            CONFIG="$2"
            shift 2
            ;;
        --use-benchmark-db)
            USE_BENCHMARK_DB=true
            shift
            ;;
        --start-services)
            START_SERVICES=true
            shift
            ;;
        --help)
            echo "Omni Perf Benchmark"
            echo ""
            echo "Usage: $0 [options]"
            echo ""
            echo "Options:"
            echo "  --num-queries N      Number of queries to run (default: 1000)"
            echo "  --concurrency N      Concurrent query execution in burst mode (default: 10)"
            echo "  --warmup N           Warmup queries (default: 100)"
            echo "  --target-qps N       Target queries per second, 0 = unlimited (default: 0)"
            echo "  --max-docs N         Maximum documents to index"
            echo "  --skip-indexing      Skip indexing, use existing data"
            echo "  --cleanup            Clean up benchmark database after run"
            echo "  --data-dir PATH      Path to prepared NQ data (default: benchmarks/data/nq_benchmark)"
            echo "  --config PATH        Config file path (default: benchmarks/config/latency.toml)"
            echo "  --use-benchmark-db   Use isolated benchmark database (omni_benchmark)"
            echo "  --start-services     Start Docker services before benchmark"
            echo "  --help               Show this help message"
            echo ""
            echo "Examples:"
            echo "  # Run with default settings (burst mode)"
            echo "  $0"
            echo ""
            echo "  # Run with rate limiting at 1.5 QPS"
            echo "  $0 --target-qps 1.5 --num-queries 500"
            echo ""
            echo "  # Run with isolated benchmark database"
            echo "  $0 --use-benchmark-db --start-services --max-docs 10000"
            echo ""
            echo "  # Skip indexing (use existing data)"
            echo "  $0 --skip-indexing --num-queries 2000"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo "========================================"
echo "Omni Perf Benchmark"
echo "========================================"
echo ""

# Start services if requested
if [ "$START_SERVICES" = true ]; then
    echo "Starting Docker services..."
    if [ "$USE_BENCHMARK_DB" = true ]; then
        echo "Using benchmark database (omni_benchmark)"
        docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml -f docker/docker-compose.benchmark.yml --env-file .env up -d
    else
        docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env up -d
    fi

    echo "Waiting for services to be ready..."
    sleep 5
fi

# Check if services are running
echo "Checking service health..."

if ! curl -sf http://localhost:3001/health > /dev/null 2>&1; then
    echo "ERROR: Searcher service not available at http://localhost:3001"
    echo "Please start services with: docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml up -d"
    exit 1
fi
echo "✓ Searcher service is healthy"

if ! curl -sf http://localhost:3002/health > /dev/null 2>&1; then
    echo "WARNING: Indexer service not available at http://localhost:3002"
    echo "Indexing may not work correctly"
else
    echo "✓ Indexer service is healthy"
fi

# Check if data exists
if [ ! -d "$DATA_DIR" ] || [ ! -f "$DATA_DIR/corpus.jsonl" ]; then
    echo ""
    echo "NQ benchmark data not found at $DATA_DIR"
    echo ""

    # Check if raw NQ data exists
    if [ -d "benchmarks/data/v1.0" ]; then
        echo "Raw NQ data found. Preparing dataset with Rust (fast)..."

        # Extract max docs value if set
        MAX_DOCS_VAL=""
        if [ -n "$MAX_DOCS" ]; then
            MAX_DOCS_VAL="${MAX_DOCS#--max-documents }"
        fi

        cargo run --release -p omni-benchmarks -- prepare-nq \
            --input-dir benchmarks/data/v1.0 \
            --output-dir "$DATA_DIR" \
            ${MAX_DOCS_VAL:+--max-documents $MAX_DOCS_VAL}
    else
        echo "ERROR: Raw NQ data not found at benchmarks/data/v1.0"
        echo "Please download first: gsutil -m cp -R gs://natural_questions/v1.0 benchmarks/data/"
        exit 1
    fi
fi

echo ""
echo "Configuration:"
echo "  Data directory: $DATA_DIR"
echo "  Config file: $CONFIG"
echo "  Queries: $NUM_QUERIES"
echo "  Warmup: $WARMUP"
if [ "$TARGET_QPS" = "0" ]; then
    echo "  Mode: Burst (concurrency: $CONCURRENCY)"
else
    echo "  Mode: Rate-limited ($TARGET_QPS QPS)"
fi
if [ -n "$SKIP_INDEXING" ]; then
    echo "  Indexing: Skipped"
fi
if [ -n "$CLEANUP" ]; then
    echo "  Cleanup: Enabled"
fi
echo ""

# Run benchmark
echo "Starting benchmark..."
cargo run --release -p omni-benchmarks -- latency \
    --config "$CONFIG" \
    --data-dir "$DATA_DIR" \
    --num-queries "$NUM_QUERIES" \
    --concurrency "$CONCURRENCY" \
    --warmup "$WARMUP" \
    --target-qps "$TARGET_QPS" \
    $MAX_DOCS \
    $SKIP_INDEXING \
    $CLEANUP

echo ""
echo "========================================"
echo "Benchmark complete!"
echo "Results saved to: benchmarks/results/latency/"
echo "========================================"
