# Omni Search Benchmarks

A comprehensive benchmarking system for evaluating Omni's hybrid search (FTS + semantic embeddings) performance, including both search relevance and latency measurements.

## Overview

This benchmarking harness provides:

### Relevance Benchmarks
- **Standard IR Benchmarks**: BEIR and MS MARCO dataset support
- **Comprehensive Metrics**: nDCG, MRR, MAP, Precision, Recall at various cutoffs
- **Search Mode Comparison**: Compare fulltext, semantic, and hybrid search performance
- **Automated Reports**: HTML and CSV reports with statistical analysis

### Latency Benchmarks
- **Performance Testing**: Measure search latency under various load conditions
- **Natural Questions Dataset**: Uses Google's NQ dataset for realistic query workloads
- **Execution Modes**: Burst mode (max throughput) or rate-limited mode (target QPS)
- **Detailed Statistics**: Min, max, mean, median, p95, p99 latencies and throughput metrics

## Quick Start

### Prerequisites

1. **Start Omni services** with Docker Compose:
   ```bash
   cd /path/to/omni
   docker compose up -d
   ```

2. **Install required tools**:
   ```bash
   # PostgreSQL client tools
   sudo apt-get install postgresql-client
   
   # Redis client tools
   sudo apt-get install redis-tools
   
   # Rust toolchain (if not already installed)
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

### Running Relevance Benchmarks

```bash
# Quick test with BEIR dataset and hybrid search
cd benchmarks
./scripts/run_benchmarks.sh

# Test all search modes
./scripts/run_benchmarks.sh beir all

# Full comprehensive benchmark (longer running)
./scripts/run_benchmarks.sh beir all false

# Get help
./scripts/run_benchmarks.sh --help
```

### Running Latency Benchmarks

```bash
cd benchmarks

# Run with default settings (burst mode, 1000 queries)
./scripts/run_latency_benchmark.sh

# Run with rate limiting at 10 QPS
./scripts/run_latency_benchmark.sh --target-qps 10 --num-queries 500

# Skip indexing (use existing data)
./scripts/run_latency_benchmark.sh --skip-indexing --num-queries 2000

# Limit document corpus size
./scripts/run_latency_benchmark.sh --max-docs 10000

# Get help
./scripts/run_latency_benchmark.sh --help
```

## What the Benchmarks Do

### Relevance Benchmarks
1. **Database Setup**: Creates a separate `omni_benchmark` database to ensure clean state
2. **Dataset Loading**: Downloads and parses benchmark datasets (BEIR/MS MARCO)
3. **Document Indexing**: Loads benchmark documents into Omni's database
4. **Query Execution**: Runs benchmark queries against different search modes
5. **Metrics Calculation**: Computes nDCG, MRR, MAP, Precision, Recall
6. **Report Generation**: Creates detailed HTML and CSV reports

### Latency Benchmarks
1. **Data Preparation**: Parses Natural Questions dataset (requires pre-download)
2. **Document Indexing**: Loads documents with embeddings into Omni
3. **Warmup Phase**: Runs warmup queries to prime caches
4. **Benchmark Phase**: Executes queries in burst or rate-limited mode
5. **Statistics Calculation**: Computes latency percentiles and throughput
6. **Results Export**: Saves detailed JSON results with per-query measurements

## Configuration

Edit `config/default.toml` to customize:

```toml
# Service URLs (should match your Docker setup)
searcher_url = "http://localhost:3001"
database_url = "postgresql://postgres:password@localhost:5432/omni_benchmark"

# Benchmark settings
max_results_per_query = 100
concurrent_queries = 5
rate_limit_delay_ms = 100

# Dataset configurations
[datasets.beir]
datasets = ["nfcorpus", "fiqa", "scifact"]  # Lightweight datasets for quick testing
```

## Available Datasets

### BEIR (Benchmarking Information Retrieval)
- **nfcorpus**: Medical/nutrition documents (small, good for testing)
- **fiqa**: Financial question answering
- **scifact**: Scientific fact verification
- **trec-covid**: COVID-19 research papers
- **nq**: Natural Questions
- **hotpotqa**: Multi-hop reasoning
- And more...

### MS MARCO
- **Passage Ranking**: Large-scale passage retrieval
- **Document Ranking**: Full document retrieval

### Custom/Enterprise
- **Synthetic Enterprise**: Generated queries for typical enterprise search scenarios
- **Custom Datasets**: Load your own query/document pairs

## Metrics Explained

### Relevance Metrics
- **nDCG@k**: Normalized Discounted Cumulative Gain - primary ranking quality metric
- **MRR**: Mean Reciprocal Rank - position of first relevant result
- **MAP@k**: Mean Average Precision - average precision across recall levels
- **Precision@k**: Fraction of retrieved docs that are relevant
- **Recall@k**: Fraction of relevant docs that are retrieved

### Latency Metrics
- **Min/Max**: Minimum and maximum query latency
- **Mean**: Average query latency
- **Median (p50)**: 50th percentile latency
- **p95/p99**: 95th and 99th percentile latencies
- **Throughput (QPS)**: Queries processed per second

## Search Modes

- **fulltext**: PostgreSQL full-text search only
- **semantic**: Vector similarity search only  
- **hybrid**: Combines fulltext and semantic search with weighted scores

## Manual Usage

You can also run benchmarks manually:

```bash
cd benchmarks

# Build the benchmark binary
cargo build --release

# Download datasets
cargo run --release -p omni-benchmarks -- setup --dataset beir

# Run relevance benchmark
cargo run --release -p omni-benchmarks -- run \
  --config config/default.toml \
  --dataset beir \
  --search-mode hybrid

# Generate reports
cargo run --release -p omni-benchmarks -- report \
  --results-dir results \
  --format html

# Prepare Natural Questions dataset (requires pre-downloaded NQ data)
cargo run --release -p omni-benchmarks -- prepare-nq \
  --input-dir data/v1.0 \
  --output-dir data/nq_benchmark \
  --max-documents 100000

# Run latency benchmark
cargo run --release -p omni-benchmarks -- latency \
  --config config/latency.toml \
  --data-dir data/nq_benchmark \
  --num-queries 1000 \
  --concurrency 10 \
  --warmup 100
```

## Results and Reports

After running benchmarks, check:

### Relevance Results
- `results/benchmark_report.html` - Interactive HTML report
- `results/benchmark_results.csv` - Raw metrics in CSV format
- `results/*.json` - Individual benchmark run results

### Latency Results
- `results/latency/*.json` - Detailed latency results with per-query measurements

## Dataset Sizes

### Relevance Datasets (BEIR/MS MARCO)
| Dataset | Queries | Documents | Download Size | Time Estimate |
|---------|---------|-----------|---------------|---------------|
| nfcorpus | 323 | 3.6K | ~2MB | 2-3 minutes |
| fiqa | 648 | 57K | ~30MB | 5-10 minutes |
| scifact | 300 | 5K | ~5MB | 2-3 minutes |
| nq (BEIR) | 3,452 | 2.7M | ~2GB | 30-60 minutes |
| msmarco | 6,980 | 8.8M | ~3GB | 60+ minutes |

### Latency Dataset (Natural Questions)
| Dataset | Queries | Documents | Download Size | Notes |
|---------|---------|-----------|---------------|-------|
| NQ (full) | 307K | 307K | ~43GB | Download via gsutil |

To download NQ data:
```bash
gsutil -m cp -R gs://natural_questions/v1.0 benchmarks/data/
```

## Troubleshooting

### Services Not Running
```bash
# Check Docker services
docker compose ps

# Check service health
curl http://localhost:3001/health
```

### Database Issues
```bash
# Check PostgreSQL connection
pg_isready -h localhost -p 5432 -U postgres

# Check if benchmark DB exists
psql -h localhost -U postgres -l | grep omni_benchmark
```

### Memory Issues
- Reduce concurrent queries: `concurrent_queries = 2`
- Use smaller datasets for testing
- Increase Docker memory limits

## Architecture

```
benchmarks/
├── src/
│   ├── config/          # Configuration management
│   ├── datasets/        # Dataset loaders (BEIR, MS MARCO, NQ, custom)
│   ├── evaluator/       # Metrics calculation and benchmark execution
│   │   ├── metrics.rs           # Relevance and latency metrics
│   │   ├── benchmark_evaluator.rs  # Relevance benchmark runner
│   │   └── latency_benchmark.rs    # Latency benchmark runner
│   ├── indexer/         # Document indexing into Omni database
│   ├── reporter/        # Report generation (HTML, CSV, JSON)
│   └── search_client/   # HTTP client for Omni searcher service
├── config/
│   ├── default.toml     # Relevance benchmark configuration
│   └── latency.toml     # Latency benchmark configuration
├── scripts/
│   ├── run_benchmarks.sh        # Relevance benchmark script
│   └── run_latency_benchmark.sh # Latency benchmark script
└── results/             # Generated reports and metrics
    └── latency/         # Latency benchmark results
```

## Contributing

To add new datasets or metrics:

1. Implement the `DatasetLoader` trait for new datasets
2. Add metrics to `MetricsCalculator`
3. Update configuration schema
4. Add tests and documentation

## License

This benchmarking system is part of the Omni project and follows the same license.