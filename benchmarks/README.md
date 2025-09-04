# Omni Search Relevance Benchmarks

A comprehensive benchmarking system for evaluating the search relevance performance of Omni's hybrid search (FTS + semantic embeddings) against standard information retrieval datasets.

## Overview

This benchmarking harness provides:

- **Standard IR Benchmarks**: BEIR and MS MARCO dataset support
- **Comprehensive Metrics**: nDCG, MRR, MAP, Precision, Recall at various cutoffs
- **Search Mode Comparison**: Compare fulltext, semantic, and hybrid search performance
- **Automated Reports**: HTML and CSV reports with statistical analysis
- **Enterprise Datasets**: Custom dataset generation for enterprise search scenarios

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

### Running Benchmarks

The easiest way is to use the provided script:

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

## What the Benchmark Does

1. **Database Setup**: Creates a separate `omni_benchmark` database to ensure clean state
2. **Dataset Loading**: Downloads and parses benchmark datasets (BEIR/MS MARCO)
3. **Document Indexing**: Loads benchmark documents into Omni's database
4. **Query Execution**: Runs benchmark queries against different search modes
5. **Metrics Calculation**: Computes nDCG, MRR, MAP, Precision, Recall
6. **Report Generation**: Creates detailed HTML and CSV reports

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

- **nDCG@k**: Normalized Discounted Cumulative Gain - primary ranking quality metric
- **MRR**: Mean Reciprocal Rank - position of first relevant result
- **MAP@k**: Mean Average Precision - average precision across recall levels
- **Precision@k**: Fraction of retrieved docs that are relevant
- **Recall@k**: Fraction of relevant docs that are retrieved

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
cargo run --release --bin benchmark -- setup --dataset beir

# Run benchmark
cargo run --release --bin benchmark -- run \
  --config config/default.toml \
  --dataset beir \
  --search-mode hybrid

# Generate reports
cargo run --release --bin benchmark -- report \
  --results-dir results \
  --format html
```

## Results and Reports

After running benchmarks, check:

- `results/benchmark_report.html` - Interactive HTML report
- `results/benchmark_results.csv` - Raw metrics in CSV format
- `results/*.json` - Individual benchmark run results

## Performance Tips

### Quick Testing
- Use lightweight BEIR datasets like `nfcorpus` or `fiqa`
- Limit to single search mode
- Reduce `max_results_per_query` to 20-50

### Comprehensive Evaluation
- Include multiple BEIR datasets
- Test all search modes for comparison
- Use statistical significance testing

### Optimization
- Use results to tune hybrid search weights
- Compare semantic embedding models
- Analyze query-specific performance

## Dataset Sizes

| Dataset | Queries | Documents | Download Size | Time Estimate |
|---------|---------|-----------|---------------|---------------|
| nfcorpus | 323 | 3.6K | ~2MB | 2-3 minutes |
| fiqa | 648 | 57K | ~30MB | 5-10 minutes |
| scifact | 300 | 5K | ~5MB | 2-3 minutes |
| nq | 3,452 | 2.7M | ~2GB | 30-60 minutes |
| msmarco | 6,980 | 8.8M | ~3GB | 60+ minutes |

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
│   ├── datasets/        # Dataset loaders (BEIR, MS MARCO, custom)
│   ├── evaluator/       # Metrics calculation and benchmark execution
│   ├── indexer/         # Document indexing into Omni database
│   ├── reporter/        # Report generation (HTML, CSV, JSON)
│   └── search_client/   # HTTP client for Omni searcher service
├── config/
│   └── default.toml     # Default configuration
├── scripts/
│   └── run_benchmarks.sh # Main automation script
└── results/             # Generated reports and metrics
```

## Contributing

To add new datasets or metrics:

1. Implement the `DatasetLoader` trait for new datasets
2. Add metrics to `MetricsCalculator`
3. Update configuration schema
4. Add tests and documentation

## License

This benchmarking system is part of the Omni project and follows the same license.