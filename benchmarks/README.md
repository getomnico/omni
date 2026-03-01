# Omni Search Benchmarks

Measures search relevance (nDCG, MRR, MAP, Precision, Recall) and query latency (p50/p95/p99, QPS) across fulltext, semantic, and hybrid search modes using standard IR datasets.

## Quick Start

Omni services must be running. If not,

```bash
docker compose -f docker/docker-compose.yml -f docker/docker-compose.benchmark.yml --env-file .env up -d
```

Then:

```bash
# Download datasets
cargo run --release -p omni-benchmarks -- setup --dataset beir

# Run benchmark against hybrid search
cargo run --release -p omni-benchmarks -- run \
  --config benchmarks/config/default.toml \
  --dataset beir \
  --search-mode hybrid

# Run all search modes with warmup and concurrency tuning
cargo run --release -p omni-benchmarks -- run \
  --config benchmarks/config/default.toml \
  --dataset beir \
  --search-mode all \
  --warmup 100 \
  --concurrency 10

# Generate HTML/CSV reports from saved results
cargo run --release -p omni-benchmarks -- report \
  --results-dir benchmarks/results \
  --format html
```

Results are saved to `results/` as JSON, HTML, and CSV.

## Configuration

Edit `config/default.toml`:

```toml
searcher_url = "http://localhost:3001"
database_url = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"

max_results_per_query = 100
concurrent_queries = 5
rate_limit_delay_ms = 100

[datasets.beir]
datasets = ["nfcorpus", "fiqa", "scifact"]  # Small datasets for quick iteration
```

## Datasets

### BEIR

Standard heterogeneous IR benchmark suite: `nfcorpus`, `fiqa`, `scifact`, `trec-covid`, `scidocs`, `nq`, `hotpotqa`, `climate-fever`, `fever`, `dbpedia-entity`, `webis-touche2020`, `quora`.

Use `nfcorpus`, `scifact`, or `fiqa` for quick iteration. Use `nq` or `hotpotqa` for thorough evaluation.

### MS MARCO

Microsoft's large-scale passage ranking dataset. Binary relevance labels only.

### Natural Questions (raw)

The NQ dataset can also be prepared from Google's raw data:

```bash
gsutil -m cp -R gs://natural_questions/v1.0 benchmarks/data/

cargo run --release -p omni-benchmarks -- prepare-nq \
  --input-dir benchmarks/data/v1.0 \
  --output-dir benchmarks/data/nq_benchmark \
  --max-documents 100000
```
