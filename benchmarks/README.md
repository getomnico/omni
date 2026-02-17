# Omni Search Benchmarks

A benchmarking system for evaluating Omni's hybrid search (FTS + semantic embeddings) performance. A single `run` command measures both search relevance and query latency in one pass.

## Overview

- **Standard IR Benchmarks**: BEIR and MS MARCO dataset support
- **Comprehensive Metrics**: nDCG, MRR, MAP, Precision, Recall at various cutoffs
- **Latency Metrics**: Per-query timing with min, mean, median, p95, p99, and throughput (QPS)
- **Search Mode Comparison**: Compare fulltext, semantic, and hybrid search
- **Automated Reports**: HTML and CSV reports with statistical analysis

## Quick Start

### Prerequisites

1. **Start Omni services** with Docker Compose:
   ```bash
   docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env up -d
   ```

2. **Rust toolchain** (if not already installed):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

### Running Benchmarks

```bash
# Download datasets first
cargo run --release -p omni-benchmarks -- setup --dataset beir

# Run benchmark (relevance + latency) with hybrid search
cargo run --release -p omni-benchmarks -- run \
  --config benchmarks/config/default.toml \
  --dataset beir \
  --search-mode hybrid

# Test all search modes with warmup and concurrency tuning
cargo run --release -p omni-benchmarks -- run \
  --config benchmarks/config/default.toml \
  --dataset beir \
  --search-mode all \
  --warmup 100 \
  --concurrency 10

# Generate reports from saved results
cargo run --release -p omni-benchmarks -- report \
  --results-dir benchmarks/results \
  --format html
```

### CLI Reference

```
benchmark <COMMAND>

Commands:
  setup       Download and prepare benchmark datasets
  run         Run benchmark evaluation (relevance + latency)
  report      Generate benchmark report
  prepare-nq  Prepare Natural Questions dataset (fast Rust implementation)
```

#### `run` options

| Flag | Default | Description |
|------|---------|-------------|
| `-c, --config` | `benchmarks/config/default.toml` | Configuration file path |
| `-d, --dataset` | `beir` | Dataset to benchmark (`beir`, `msmarco`) |
| `-s, --search-mode` | `all` | Search mode (`fulltext`, `semantic`, `hybrid`, `all`) |
| `--warmup` | `50` | Warmup queries (not measured) |
| `--concurrency` | from config | Concurrent query execution |

## How It Works

1. **Database Setup**: Creates a separate `omni_benchmark` database
2. **Dataset Loading**: Downloads and parses benchmark datasets (BEIR/MS MARCO)
3. **Document Indexing**: Loads benchmark documents into Omni via the connector event queue
4. **Warmup Phase**: Runs warmup queries to prime caches (not measured)
5. **Benchmark Phase**: Executes queries concurrently, capturing both relevance results and per-query latency
6. **Results**: Computes unified metrics (relevance + latency) and saves to JSON

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

The standard heterogeneous IR benchmark suite. Each sub-dataset covers a different domain.

| Dataset | Domain | Queries | Corpus | Download |
|---------|--------|---------|--------|----------|
| nfcorpus | Biomedical / nutrition | ~323 | ~3.6K | ~2MB |
| fiqa | Financial QA | ~648 | ~57K | ~30MB |
| scifact | Scientific fact-checking | ~300 | ~5K | ~5MB |
| trec-covid | COVID-19 research | ~50 | ~171K | ~70MB |
| scidocs | Computer science papers | ~1K | ~25K | ~40MB |
| nq | Natural Questions (Wikipedia) | ~3.4K | ~2.6M | ~2GB |
| hotpotqa | Multi-hop reasoning | ~7.4K | ~5.2M | ~3GB |
| climate-fever | Climate claims | ~1.5K | ~5.4M | ~3GB |
| fever | Wikipedia fact verification | ~6.6K | ~5.4M | ~3GB |
| dbpedia-entity | Entity search | ~400 | ~4.6M | ~3GB |
| webis-touche2020 | Argument retrieval | ~49 | ~382K | ~300MB |
| quora | Duplicate question detection | ~10K | ~523K | ~50MB |

For quick iteration, use the smaller datasets (`nfcorpus`, `scifact`, `fiqa`). For thorough evaluation, use `nq` or `hotpotqa`.

### MS MARCO

Microsoft's large-scale passage ranking dataset from Bing search queries.

| Variant | Queries (dev) | Corpus | Download |
|---------|---------------|--------|----------|
| Passage | ~6.9K | ~8.8M | ~3GB |

Binary relevance labels only (relevant / not relevant).

## Metrics

### Relevance
- **nDCG@k**: Normalized Discounted Cumulative Gain — primary ranking quality metric
- **MRR**: Mean Reciprocal Rank — how high the first relevant result ranks
- **MAP@k**: Mean Average Precision — average precision across recall levels
- **Precision@k**: Fraction of top-k results that are relevant
- **Recall@k**: Fraction of all relevant docs found in top-k

### Latency
- **Min / Max / Mean**: Basic latency distribution
- **Median (p50)**: 50th percentile latency
- **p95 / p99**: Tail latencies
- **Throughput (QPS)**: Queries per second over the benchmark duration

## Search Modes

- **fulltext**: ParadeDB BM25 full-text search only
- **semantic**: pgvector cosine similarity search only
- **hybrid**: Combines BM25 + vector similarity with weighted scores

## Results

After running benchmarks, results are saved to:

- `results/<dataset>_<mode>_<timestamp>_results.json` — unified JSON with both relevance and latency metrics
- `results/benchmark_report.html` — interactive HTML report (via `report` command)
- `results/benchmark_results.csv` — CSV export (via `report` command)

## Preparing Natural Questions Data

The NQ dataset requires downloading Google's raw data and converting it:

```bash
# Download raw NQ data (~43GB)
gsutil -m cp -R gs://natural_questions/v1.0 benchmarks/data/

# Prepare benchmark format
cargo run --release -p omni-benchmarks -- prepare-nq \
  --input-dir benchmarks/data/v1.0 \
  --output-dir benchmarks/data/nq_benchmark \
  --max-documents 100000
```

## Troubleshooting

### Services Not Running
```bash
docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env ps
curl http://localhost:3001/health
```

### Database Issues
```bash
pg_isready -h localhost -p 5432 -U postgres
psql -h localhost -U postgres -l | grep omni_benchmark
```

### Memory Issues
- Reduce `concurrent_queries` in config
- Use smaller datasets (`nfcorpus`, `scifact`) for testing
- Increase Docker memory limits

## Architecture

```
benchmarks/
├── src/
│   ├── config/          # Configuration management
│   ├── datasets/        # Dataset loaders (BEIR, MS MARCO, NQ, custom)
│   ├── evaluator/       # Unified benchmark runner and metrics
│   │   ├── benchmark_evaluator.rs  # Query execution with timing
│   │   └── metrics.rs              # Relevance + latency metrics
│   ├── indexer/         # Document indexing into Omni database
│   ├── prepare_nq.rs   # NQ dataset preparation (HTML → text)
│   ├── reporter/        # Report generation (HTML, CSV, JSON)
│   └── search_client/   # HTTP client for Omni searcher service
├── config/
│   └── default.toml     # Benchmark configuration
├── scripts/
│   └── run_benchmarks.sh
└── results/             # Generated reports and metrics
```
