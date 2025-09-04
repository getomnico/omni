#!/bin/bash

# Clio Search Relevance Benchmark Runner
# This script runs benchmarks against Docker services

set -e

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BENCHMARK_ROOT="$(dirname "$PROJECT_DIR")"
CONFIG_FILE="${PROJECT_DIR}/config/default.toml"
RESULTS_DIR="${PROJECT_DIR}/results"
RUST_LOG=info

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Function to start Clio services with benchmark configuration
start_benchmark_services() {
    log_info "Starting Clio services with benchmark database configuration..."
    
    cd "$BENCHMARK_ROOT"
    
    # Start services with benchmark configuration
    log_info "Using docker compose files: docker-compose.yml + docker-compose.dev.yml + docker-compose.benchmark.yml"
    
    if docker compose \
        -f docker/docker-compose.yml \
        -f docker/docker-compose.dev.yml \
        -f docker/docker-compose.benchmark.yml \
        up -d; then
        log_success "Benchmark services started successfully"
    else
        log_error "Failed to start benchmark services"
        exit 1
    fi
}

# Function to check if Clio services are running
check_omni_services() {
    log_info "Checking Clio services..."
    
    # Check PostgreSQL
    if ! docker exec omni-postgres pg_isready -h localhost -U omni_dev > /dev/null 2>&1; then
        log_error "PostgreSQL is not running at localhost:5432"
        log_info "Starting Clio services with benchmark configuration..."
        start_benchmark_services
        sleep 5  # Give services time to start
    fi
    
    # Check Redis
    if ! docker exec omni-redis redis-cli ping > /dev/null 2>&1; then
        log_error "Redis is not running at localhost:6379"
        log_info "Starting Clio services with benchmark configuration..."
        start_benchmark_services
        sleep 5  # Give services time to start
    fi
    
    # Check if searcher service is running
    log_info "Waiting for searcher service to be ready..."
    local max_attempts=30
    local attempt=1
    
    while [ $attempt -le $max_attempts ]; do
        if curl -f -s "http://localhost:3001/health" > /dev/null 2>&1; then
            log_success "Clio searcher service is ready"
            break
        fi
        
        if [ $attempt -eq $max_attempts ]; then
            log_error "Clio searcher service is not responding at http://localhost:3001"
            log_info "Please ensure services started correctly or try manually: docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml -f docker/docker-compose.benchmark.yml up -d"
            exit 1
        fi
        
        log_info "Attempt $attempt/$max_attempts - waiting for searcher..."
        sleep 2
        ((attempt++))
    done
    
    log_success "All Clio services are running and ready"
}

# Function to setup benchmark database
setup_benchmark_database() {
    log_info "Setting up benchmark database..."
    
    # Create omni_benchmark database if it doesn't exist
    docker exec omni-postgres psql -U omni_dev -c "CREATE DATABASE omni_benchmark;" 2>/dev/null || true
    
    # Run migrations on benchmark database using Docker container
    cd "$BENCHMARK_ROOT"
    
    log_info "Running database migrations using Docker container..."
    
    if docker run --rm \
        --network host \
        -e DATABASE_URL="postgresql://omni_dev:omni_dev_password@localhost:5432/omni_benchmark" \
        -v "$BENCHMARK_ROOT/services/migrations:/migrations" \
        $(docker build -q -f services/migrations/Dockerfile .); then
        log_success "Benchmark database setup completed"
    else
        log_error "Failed to setup benchmark database"
        exit 1
    fi
}

# Function to build benchmark binary
build_benchmark() {
    log_info "Building benchmark binary..."
    cd "$PROJECT_DIR"
    
    if cargo build; then
        log_success "Benchmark binary built successfully"
    else
        log_error "Failed to build benchmark binary"
        exit 1
    fi
}

# Function to setup datasets
setup_datasets() {
    local dataset="$1"
    log_info "Setting up datasets: $dataset"
    
    cd "$PROJECT_DIR"
    if RUST_LOG="${RUST_LOG}" RUST_BACKTRACE=1 "$BENCHMARK_ROOT/target/debug/benchmark" setup --dataset "$dataset"; then
        log_success "Dataset setup completed: $dataset"
    else
        log_error "Failed to setup dataset: $dataset"
        exit 1
    fi
}

# Function to run benchmarks
run_benchmark() {
    local dataset="$1"
    local search_mode="$2"
    
    log_info "Running benchmark - Dataset: $dataset, Mode: $search_mode"
    
    cd "$PROJECT_DIR"
    if RUST_LOG="${RUST_LOG}" RUST_BACKTRACE=1 "$BENCHMARK_ROOT/target/debug/benchmark" run \
        --config "$CONFIG_FILE" \
        --dataset "$dataset" \
        --search-mode "$search_mode"; then
        log_success "Benchmark completed - Dataset: $dataset, Mode: $search_mode"
    else
        log_error "Benchmark failed - Dataset: $dataset, Mode: $search_mode"
        return 1
    fi
}

# Function to generate reports
generate_reports() {
    log_info "Generating benchmark reports..."
    
    cd "$PROJECT_DIR"
    
    # Generate HTML report
    if RUST_LOG="${RUST_LOG}" RUST_BACKTRACE=1 "$BENCHMARK_ROOT/target/debug/benchmark" report \
        --results-dir "$RESULTS_DIR" \
        --format html; then
        log_success "HTML report generated"
    else
        log_warning "Failed to generate HTML report"
    fi
    
    # Generate CSV report
    if RUST_LOG="${RUST_LOG}" RUST_BACKTRACE=1 "$BENCHMARK_ROOT/target/debug/benchmark" report \
        --results-dir "$RESULTS_DIR" \
        --format csv; then
        log_success "CSV report generated"
    else
        log_warning "Failed to generate CSV report"
    fi
}

# Function to run comprehensive benchmark suite
run_comprehensive_benchmark() {
    local datasets=("$@")
    local search_modes=("fulltext" "semantic" "hybrid")
    
    log_info "Running comprehensive benchmark suite"
    log_info "Datasets: ${datasets[*]}"
    log_info "Search modes: ${search_modes[*]}"
    
    # Setup datasets
    for dataset in "${datasets[@]}"; do
        setup_datasets "$dataset"
    done
    
    # Run benchmarks for each combination
    local total_runs=$((${#datasets[@]} * ${#search_modes[@]}))
    local current_run=0
    
    for dataset in "${datasets[@]}"; do
        for search_mode in "${search_modes[@]}"; do
            current_run=$((current_run + 1))
            log_info "Progress: $current_run/$total_runs"
            
            if ! run_benchmark "$dataset" "$search_mode"; then
                log_warning "Skipping failed benchmark: $dataset-$search_mode"
            fi
        done
    done
}

# Main script logic
main() {
    log_info "Starting Clio Search Relevance Benchmark"
    log_info "Benchmark directory: $PROJECT_DIR"
    log_info "Clio root directory: $BENCHMARK_ROOT"
    log_info "Results directory: $RESULTS_DIR"
    
    # Parse command line arguments
    DATASET="${1:-beir}"
    SEARCH_MODE="${2:-hybrid}"
    QUICK_MODE="${3:-true}"
    
    # Create results directory
    mkdir -p "$RESULTS_DIR"
    
    # Check prerequisites and setup
    check_omni_services
    setup_benchmark_database
    build_benchmark
    
    if [ "$QUICK_MODE" = "true" ]; then
        log_info "Running in quick mode - single dataset/mode"
        setup_datasets "$DATASET"
        run_benchmark "$DATASET" "$SEARCH_MODE"
    else
        log_info "Running comprehensive benchmark suite"
        
        if [ "$DATASET" = "all" ]; then
            # Use lightweight datasets for comprehensive testing
            run_comprehensive_benchmark "beir"
        else
            run_comprehensive_benchmark "$DATASET"
        fi
    fi
    
    # Generate reports
    generate_reports
    
    log_success "Benchmark suite completed successfully!"
    log_info "Results available in: $RESULTS_DIR"
    log_info "Open $RESULTS_DIR/benchmark_report.html to view detailed results"
    
    # Show final summary
    log_info ""
    log_info "=== Next Steps ==="
    log_info "1. Review the HTML report: $RESULTS_DIR/benchmark_report.html"
    log_info "2. Check detailed JSON results in: $RESULTS_DIR/"
    log_info "3. Compare different search modes and datasets"
    log_info "4. Use results to optimize your hybrid search weights"
}

# Help function
show_help() {
    echo "Clio Search Relevance Benchmark Runner"
    echo ""
    echo "Usage: $0 [DATASET] [SEARCH_MODE] [QUICK_MODE]"
    echo ""
    echo "Arguments:"
    echo "  DATASET     Dataset to benchmark (beir, msmarco, custom) [default: beir]"
    echo "  SEARCH_MODE Search mode to test (fulltext, semantic, hybrid, all) [default: hybrid]"
    echo "  QUICK_MODE  Run single dataset/mode combination (true, false) [default: true]"
    echo ""
    echo "Examples:"
    echo "  $0                    # Quick test with BEIR dataset and hybrid search"
    echo "  $0 beir all true      # Test all search modes with BEIR dataset"
    echo "  $0 beir hybrid false  # Comprehensive benchmark with BEIR (longer)"
    echo ""
    echo "Prerequisites:"
    echo "  1. Ensure you have:"
    echo "     - Rust toolchain installed"
    echo "     - PostgreSQL client tools (pg_isready, createdb)"
    echo "     - Redis client tools (redis-cli)"
    echo "     - Internet connection for dataset downloads"
    echo ""
    echo "What this script does:"
    echo "  1. Automatically starts Clio services with benchmark database configuration"
    echo "  2. Creates a separate 'omni_benchmark' database"
    echo "  3. Downloads and loads benchmark datasets"
    echo "  4. Indexes the dataset documents into Clio"
    echo "  5. Runs search queries and measures performance"
    echo "  6. Generates detailed HTML and CSV reports"
    echo ""
    echo "The benchmark uses a separate database to ensure clean state"
    echo "and won't interfere with your main Clio data."
}

# Check for help flag
if [ "$1" = "-h" ] || [ "$1" = "--help" ]; then
    show_help
    exit 0
fi

# Run main function
main "$@"
