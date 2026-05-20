# Omni EnterpriseRAG-Bench Runbook

This directory contains the reproducible driver scripts used to benchmark Omni
against [EnterpriseRAG-Bench](https://github.com/onyx-dot-app/EnterpriseRAG-Bench)
v1.0.

EnterpriseRAG-Bench v1.0 includes 511,962 synthetic enterprise documents across
9 source types and 500 questions across 10 categories.

## What Is Measured

| Metric | Definition |
|---|---|
| `correctness` | Binary LLM judge decision: does the candidate answer match the gold answer? |
| `completeness_pct` | Percent of benchmark `answer_facts` present in the candidate answer. |
| `document_recall_pct` | `len(returned_doc_ids ∩ gold_doc_ids) / len(gold_doc_ids) * 100`. |
| `invalid_extra_docs` | Returned docs that are neither gold nor judged relevant by correction. |
| `overall_score` | `mean(completeness_pct if answer_correct else 0)`. This is the leaderboard headline metric. |

Correction is enabled by default, matching the public leaderboard behavior. The
correction flow can expand the accepted document set when the system returns a
document that is relevant but missing from the original gold set.

## Repository Layout

```text
enterprise-benchmark/
├── README.md
├── load_corpus.py              # Bulk-load EnterpriseRAG-Bench docs into Omni
├── filter_questions.py         # Optional question subset/sampling helper
├── run_search.py               # Direct search endpoint driver
├── run_rag.py                  # One-shot retrieval + LLM answer driver
├── run_agentic.py              # Agentic chat-loop benchmark driver
├── run_full_pipeline.py        # Answer generation + eval orchestration
├── seed_deepseek_provider.py   # Seed DeepSeek model provider rows
├── eval_fork/                  # EnterpriseRAG-Bench eval patch
├── pyproject.toml
└── uv.lock
```

Ad-hoc analysis scripts, local reports, generated outputs, and old exploratory
material live outside this directory under `enterprise-benchmark-ad-hoc/`.

Ignored local output directories:

```text
enterprise-benchmark/data/
enterprise-benchmark/answer_evaluation/
enterprise-benchmark/runs/
```

## Architecture

```text
                            benchmark host
┌────────────────────────────────────────────────────────────────────┐
│ Postgres / ParadeDB ── Redis                                       │
│        ↑                                                           │
│ omni-indexer ── omni-searcher ── omni-ai                           │
│                    ↑              ↓                                │
│ enterprise-benchmark scripts    LLM provider                       │
│                    ↓                                               │
│            EnterpriseRAG-Bench eval                                │
└────────────────────────────────────────────────────────────────────┘
                         ↓
               OpenAI-compatible embedding endpoint
               `BAAI/bge-large-en-v1.5`, 1024 dimensions
```

The benchmark uses Omni's normal ingestion path after the connector boundary:
`load_corpus.py` inserts content blobs and connector queue events, then
`omni-indexer` writes documents and queues embeddings. `omni-ai` processes
embeddings through the configured embedding provider.

omni-web and omni-connector-manager are not required for this benchmark.

## Models

| Role | Model | Provider | Notes |
|---|---|---|---|
| Agent / answer generation | `deepseek-v4-pro` | DeepSeek | Main full-500 agent model for the canonical run. |
| Judge | `deepseek-v4-pro` | DeepSeek | OpenAI-compatible judge via the eval fork. |
| Embeddings | `BAAI/bge-large-en-v1.5` | Self-hosted TEI | 1024-dimensional embeddings served from a GPU workstation. |

For publication, the final reported artifact includes a small GPT-5.4
adjustment layer. That is separate from the clean base run:

1. Base: one full 500-question DeepSeek V4 Pro agentic run.
2. Patch: targeted GPT-5.4 remediation or re-judgement rows.
3. Merge: deterministic script or manifest showing exactly which rows changed.

## Path Conventions

The commands below use these shell variables so the runbook is independent of a
specific host layout:

```bash
export OMNI_REPO=/path/to/omni
export ERAG_REPO=/path/to/EnterpriseRAG-Bench
export UV=uv
```

If `uv` is installed somewhere else, point `UV` at the full path:

```bash
export UV=/path/to/uv
```

Default local service URLs used by the scripts:

| Service | URL |
|---|---|
| Searcher | `http://localhost:3001` |
| AI | `http://localhost:3003` |
| Embeddings | OpenAI-compatible endpoint configured during corpus load |

## 1. Prepare the Omni Stack

Clone the benchmark branch:

```bash
git clone --branch enterprise-benchmark --single-branch \
  https://github.com/getomnico/omni.git "$OMNI_REPO"
cd "$OMNI_REPO"
```

Clone EnterpriseRAG-Bench:

```bash
git clone https://github.com/onyx-dot-app/EnterpriseRAG-Bench.git "$ERAG_REPO"
cd "$ERAG_REPO"
$UV venv .venv
$UV pip install -r requirements.txt
```

Create the runtime env file:

```bash
cp .env.example .env
```

Required values:

```bash
DATABASE_USERNAME=omni_bench
DATABASE_PASSWORD=omni_bench_password
DATABASE_NAME=omni_benchmark
ENCRYPTION_KEY=bench-encryption-key-must-be-32-chars-or-more
ENCRYPTION_SALT=bench-salt-16chr
SEARCHER_PORT=3001
INDEXER_PORT=3002
AI_SERVICE_PORT=3003
BENCHMARK_MODE=true
SEARCH_MODE=hybrid
AGENT_MAX_ITERATIONS=150
```

Bring up the benchmark stack:

```bash
docker compose \
  -f docker/docker-compose.yml \
  -f docker/docker-compose.benchmark.yml \
  --env-file .env up -d
```

Health checks:

```bash
curl -s http://localhost:3001/health | jq .
curl -s http://localhost:3003/health | jq .
```

## 2. Configure Providers

Store benchmark API keys in the EnterpriseRAG-Bench checkout, not in the repo:

```bash
# $ERAG_REPO/.env
DEEPSEEK_API_KEY=...
OPENAI_API_KEY=...
```

Seed DeepSeek as Omni's default LLM provider:

```bash
cd "$OMNI_REPO/enterprise-benchmark"
set -a && . "$ERAG_REPO/.env" && set +a
$UV run python seed_deepseek_provider.py
```

## 3. Configure an Embedding Endpoint

Omni needs an OpenAI-compatible embeddings endpoint. It can be a cloud provider,
a hosted internal service, or a local server. The endpoint must expose
`/v1/embeddings` and return vectors with the dimensionality configured in Omni.

For our run, we used `BAAI/bge-large-en-v1.5` with 1024-dimensional embeddings.
Keep these values handy; they are passed to `load_corpus.py` when seeding Omni's
embedding provider row:

```bash
export EMBEDDING_API_URL=http://<embedding-host>:<port>/v1
export EMBEDDING_API_KEY=<embedding-api-key-if-required>
```

TEI is one valid local option:

```bash
docker run -d --gpus all \
  -p 8090:80 \
  -v $PWD/data:/data \
  --name tei-bge-large \
  ghcr.io/huggingface/text-embeddings-inference:1.5 \
  --model-id BAAI/bge-large-en-v1.5
```

The only networking requirement is that the benchmark host can reach the
embedding base URL:

```text
$EMBEDDING_API_URL
```

`load_corpus.py` seeds the embedding provider row with this URL.

## 4. Download and Load the Corpus

Download EnterpriseRAG-Bench v1.0 into:

```text
$OMNI_REPO/enterprise-benchmark/data/
```

Expected dir structure:

```text
enterprise-benchmark/data/questions.jsonl
enterprise-benchmark/data/<source_type>/dsid_*.txt
```

Load the corpus:

```bash
cd "$OMNI_REPO/enterprise-benchmark"
$UV sync
$UV run python load_corpus.py \
  --data-dir data \
  --embedding-provider openai \
  --embedding-model BAAI/bge-large-en-v1.5 \
  --embedding-dimensions 1024 \
  --embedding-api-url "$EMBEDDING_API_URL" \
  --embedding-api-key "$EMBEDDING_API_KEY"
```

Wait for both queues to drain:

```sql
SELECT status, count(*) FROM connector_events_queue GROUP BY status;
SELECT status, count(*) FROM embedding_queue GROUP BY status;
```

The full corpus embedding could take hours. Semantic and hybrid search should not
be used for final runs until `embedding_queue` is drained.

## 5. Install the Eval Fork

EnterpriseRAG-Bench's stock evaluator uses OpenAI's Responses API. The fork in
`eval_fork/` adds an OpenAI-compatible chat-completions client so DeepSeek can be
used as the judge.

```bash
cd "$OMNI_REPO/enterprise-benchmark"
bash eval_fork/install.sh "$ERAG_REPO"
```

## 6. Run Preflight Checks

Run only the health checks before spending tokens:

```bash
cd "$OMNI_REPO/enterprise-benchmark"
set -a && . "$ERAG_REPO/.env" && set +a
$UV run python run_full_pipeline.py \
  --suffix healthcheck \
  --samples 1 \
  --skip-gen \
  --skip-eval
```

This checks:

- `omni-ai` health
- `omni-searcher` health
- a semantic-only search through `/search`

To also probe the embedding endpoint directly from the benchmark host, pass:

```bash
$UV run python run_full_pipeline.py \
  --suffix healthcheck \
  --samples 1 \
  --skip-gen \
  --skip-eval \
  --embedding-url "$EMBEDDING_API_URL" \
  --embedding-model BAAI/bge-large-en-v1.5 \
  --embedding-dimensions 1024
```

Direct embedding and llama.cpp endpoint probes are skipped unless their URLs are
provided. The semantic search probe still verifies that Omni itself can perform
semantic retrieval.

## 7. Run the Full Agentic Benchmark

Use a fresh run directory for every full run:

```bash
cd "$OMNI_REPO/enterprise-benchmark"
set -a && . "$ERAG_REPO/.env" && set +a
nohup $UV run python run_full_pipeline.py \
  --questions data/questions.jsonl \
  --samples 500 \
  --suffix omni_agentic_deepseek_v4_pro_full500 \
  --run-name omni_agentic_deepseek_v4_pro_full500 \
  --concurrency 4 \
  --eval-parallelism 12 \
  --judge-model deepseek-v4-pro \
  --judge-base-url https://api.deepseek.com/v1 \
  --judge-api-key-env DEEPSEEK_API_KEY \
  > full500.out 2>&1 &
```

Correction is enabled by default. Add `--no-correction` only for a cheaper sanity
check that should not be compared to leaderboard results.

Run artifacts are written under:

```text
enterprise-benchmark/runs/<run_name>_<timestamp>/
```

Typical files:

```text
answers_<system_name>.jsonl
results_<system_name>.json
question_chat_map.json
run_metadata.json
```

## 8. Resume or Re-run Parts

Answer generation only:

```bash
$UV run python run_full_pipeline.py \
  --questions data/questions.jsonl \
  --samples 500 \
  --suffix answer_gen_only \
  --skip-eval
```

Evaluation only:

```bash
$UV run python run_full_pipeline.py \
  --questions data/questions.jsonl \
  --samples 500 \
  --suffix eval_only \
  --run-dir runs/<existing_run_dir> \
  --skip-gen \
  --judge-model deepseek-v4-pro
```

Direct eval command:

```bash
cd "$ERAG_REPO"
set -a && . .env && set +a
export LLM_PROVIDER=openai_compat
export LLM_API_KEY=$DEEPSEEK_API_KEY
export LLM_BASE_URL=https://api.deepseek.com/v1
export LLM_MODEL_NAME=deepseek-v4-pro

.venv/bin/python -m src.scripts.answer_evaluation.metrics_based_eval \
  --answers-file "$OMNI_REPO/enterprise-benchmark/runs/<run>/answers_<system>.jsonl" \
  --results-file "$OMNI_REPO/enterprise-benchmark/runs/<run>/results_<system>.json" \
  --parallelism 12
```

## 9. Read Final Metrics

```bash
cat "$OMNI_REPO/enterprise-benchmark/runs/<run>/results_<system>.json" \
  | jq '.aggregate_stats'
```

Important aggregate fields:

- `combined_correctness_completeness_score`
- `average_correctness_pct`
- `average_completeness_pct`
- `average_recall_pct`
- `average_invalid_extra_docs`
- `total_questions`

Category metrics:

```bash
cat "$OMNI_REPO/enterprise-benchmark/runs/<run>/results_<system>.json" \
  | jq '.question_type_stats'
```

## Operational Notes

- Keep `SEARCH_MODE=hybrid` for the main agentic run.
- Use `--concurrency 4` as a conservative upper bound for the searcher unless
  you have load-tested higher concurrency on your hardware.
- Use `AGENT_MAX_ITERATIONS=150` for long-running agentic questions.
- The benchmark evaluator can make multiple LLM calls per answer because document
  correction is enabled.
- DeepSeek exposes cache read usage via provider-specific usage fields; do not
  assume all OpenAI-compatible endpoints use identical token field names.
- The evaluator compares submitted docs using EnterpriseRAG-Bench `dsid_*`
  document IDs. Internal Omni document IDs are not valid benchmark citations.
