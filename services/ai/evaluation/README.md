# RAG Evaluation Framework

Measures retrieval and generation quality of the Omni AI service using [RAGAS](https://docs.ragas.io/) metrics against a golden dataset built from [MRQA](https://github.com/mrqa/MRQA-Shared-Task-2019).

Scores are persisted to the `eval_scores` Postgres table and printed to the terminal with PASS/FAIL against configurable thresholds.

---

## Architecture

```
evaluation/
├── config.py              # EvalConfig — thresholds and judge model, env-configurable
├── models.py              # EvalTrace, EvalScore pydantic models
├── store.py               # DB persistence + isolated migration runner
├── datasets/
│   ├── generate_golden.py # CLI: downloads MRQA from HuggingFace → golden_set.yaml + corpus/
│   ├── golden_set.yaml    # ~20-entry golden set (committed, auto-generated)
│   └── corpus/            # Generated .txt files (~5000) for filesystem connector (gitignored)
├── runners/
│   ├── runner.py            # Synthetic mode; _score_samples() RAGAS adapter
│   └── chat_loop_runner.py  # Drives the production agent loop end-to-end
├── reporters/
│   └── console.py         # Tabular PASS/FAIL terminal output
└── migrations/
    └── 01_create_evaluation_tables.sql  # Isolated schema (eval_scores, eval_runs)
```

### Evaluation modes

| Mode                | How context is sourced                                                                                 | Requires                                 |
| ------------------- | ------------------------------------------------------------------------------------------------------ | ---------------------------------------- |
| **Synthetic** | Oracle NQ context from `golden_set.yaml`                                                             | LLM API key + DB                         |
| **Chat loop** | Drives `run_agent_loop` (from `services/ai/agent_loop.py`, shared with the chat router) end-to-end | Omni searcher running + LLM API key + DB |

---

## Prerequisites

```bash
uv sync --extra eval --project services/ai
```

The runner defaults to the dev Postgres from
[`docker-compose.dev.yml`](../../../docker/docker-compose.dev.yml) (host-exposed
on `localhost:5432`, credentials `omni_dev`/`omni_dev_password`/`omni_dev`) —
the same DB the AI service uses, so it can read the `models` table and write
eval scores back. No `DATABASE_*` exports needed unless you want to point at
a different DB. Change `DATABASE_*` variables if you evaluate other instance.

Only the eval-specific vars need to be set:

```bash
export EVAL_OPENAI_API_KEY=sk-...
export EVAL_OPENAI_API_BASE=https://openrouter.ai/api/v1
export EVAL_MODEL=google/gemma-4-31b-it
export EVAL_SEARCHER_URL=http://localhost:3001
```

- `EVAL_OPENAI_API_KEY` — judge-model API key (OpenAI or OpenRouter).
- `EVAL_OPENAI_API_BASE` — optional, defaults to `https://api.openai.com`.
- `EVAL_MODEL` — judge model.
- `EVAL_SEARCHER_URL` — omni-searcher URL.

`PORT`, `MODEL_PATH`, `REDIS_URL`, and `CONNECTOR_MANAGER_URL` are required by
the AI service's `config.py` but never touched by the runner at runtime —
they're defaulted to placeholders inside `chat_loop_runner.py`.

---

## Step-by-step guide

### 1. Generate the golden dataset and corpus files

Downloads the MRQA validation split from HuggingFace (cached after first run). No LLM key required.

```bash
uv run --directory services/ai python -m evaluation.datasets.generate_golden \
    --corpus-size 5000 --golden-size 20
```

This produces two outputs:

- `evaluation/datasets/corpus/` — ~5000 `.txt` files (the retrieval haystack, deduplicated by content hash)
- `evaluation/datasets/golden_set.yaml` — ~20 queries with answer, oracle passage, and `reference_doc_ids` pointing into the corpus

The corpus is sized for retrieval to be non-trivial; the golden set stays small so each query can be inspected by hand. Both are gitignored (generated).

`--corpus-size` (default `5000`) and `--golden-size` (default `20`, must be ≤ corpus-size) are independent. Golden queries are deterministic for a fixed `--corpus-size` and stratified across MRQA subsets.

Wait until embeddings processed.

#### Configure the filesystem source for offline evaluation

The corpus files must be ingested into Omni before offline mode will produce non-zero scores. The `docker-compose.dev.yml` mounts `corpus/` into the filesystem connector container at `/data/eval-corpus`.

1. Start the stack with the `filesystem` profile:
   ```bash
   docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env --profile filesystem up -d
   ```
2. In the Omni admin UI → **Sources** → **Add source** → **File System**
   - Set **Base path** to `/data/eval-corpus`
   - Save and trigger a sync
3. Wait for the sync to complete (check the Sources page or searcher logs)

### 2. Run the chat-loop evaluation

Drive the production agent loop end-to-end against the golden set:

```bash
uv run --directory services/ai python -m evaluation.runners.chat_loop_runner
```

The agent model is loaded from the platform's default model in the DB (the
same one the chat router uses) — configure it in the admin UI under
**Settings → Models**. This invokes `run_agent_loop` for each golden query,
then scores the resulting (query, contexts, response) tuple with RAGAS via
`_score_samples`.

#### Current results

Metric            Score    Threshold  Status

---

context_recall   0.35          0.4  FAIL
faithfulness     0.45          0.5  FAIL

---

## Configuration

All thresholds and the judge model are configurable via environment variables:

| Variable                            | Default               | Description                             |
| ----------------------------------- | --------------------- | --------------------------------------- |
| `EVAL_MODEL`                      | google/gemma-4-31b-it | Judge LLM (any OpenAI-compatible model) |
| `EVAL_PROVIDER`                   | `openai`            | Provider hint                           |
| `EVAL_FAITHFULNESS_THRESHOLD`     | `0.5`               | Minimum faithfulness score (0–1)       |
| `EVAL_CONTEXT_RECALL_THRESHOLD`   | `0.4`               | Minimum context recall score (0–1)     |
| `EVAL_ANSWER_RELEVANCY_THRESHOLD` | `0.5`               | Minimum answer relevancy score (0–1)   |

---

## Metrics

| Metric             | Category   | What it measures                                            |
| ------------------ | ---------- | ----------------------------------------------------------- |
| `faithfulness`   | Generation | Are claims in the answer grounded in the retrieved context? |
| `context_recall` | Retrieval  | Does the retrieved context cover the reference answer?      |

Both metrics are scored 0–1 by an LLM judge (RAGAS framework). Scores are comparable to published RAGAS benchmarks because the same NQ dataset is used.

### `faithfulness` — is the answer grounded in what was retrieved?

**Diagnoses hallucination in the generation step.** The judge splits the
generated answer into atomic factual claims, then for each claim asks whether
it can be inferred from the retrieved context. Score is the fraction of
claims that are supported:

```
faithfulness = supported_claims / total_claims
```

A score of `1.0` means every statement in the answer is traceable to the
retrieved passages. A score of `0.3` means most of what the model said was
invented (or pulled from its parametric memory rather than the corpus).

**Only inspects answer ↔ context.** It does not care whether the answer is
*correct* — a faithful answer can still be wrong if retrieval surfaced the
wrong documents. Pair with `context_recall` to separate "model made it up"
from "we never gave the model the right source."

**Typical failure modes a low score flags:** the model fills gaps from
pre-training, over-generalizes from a single passage, or invents specifics
(dates, numbers, names) that the context didn't contain.

### `context_recall` — did retrieval surface the evidence needed to answer?

**Diagnoses coverage gaps in the retrieval step.** The judge splits the
*reference* answer (from the golden set, not the model's output) into atomic
claims, then for each one asks whether the retrieved context contains enough
to support it:

```
context_recall = reference_claims_covered / reference_claims_total
```

A score of `1.0` means the retriever handed the LLM everything needed to
produce the gold answer. A score of `0.3` means the retriever missed most of
the relevant evidence — even a perfect generator couldn't answer correctly
from what it was given.

**Only inspects reference_answer ↔ context.** It ignores the model's own
output entirely; it's a pure retrieval metric. Low `context_recall` with
high `faithfulness` is the classic "confidently wrong" pattern — the model
stayed grounded in what it was shown, but what it was shown was incomplete.

**Typical failure modes a low score flags:** embeddings miss semantic
paraphrases, BM25 misses lexical variants, top-k cutoff is too aggressive,
the right document isn't indexed, or chunking split the answer across
boundaries so no single chunk carries the full evidence.

---

## Database schema

The eval schema lives in `migrations/01_create_evaluation_tables.sql` and is applied automatically by `store.ensure_eval_schema()` when the runner starts. It is **isolated from the production schema** — these tables are never touched by the main Rust migration runner.

Key tables:

- `eval_scores` — one row per (trace, metric) pair
- `eval_runs` — one row per batch evaluation run with aggregate summary

---

## Adding a new metric

1. Add the RAGAS metric import to `runners/runner.py` and include it in the `evaluate()` call.
2. Add a threshold field to `EvalConfig` in `config.py`.
3. Add the metric name to the `for metric_name in (...)` loop in `_score_samples()`.
4. Add a threshold assertion test to `tests/evaluation/test_runner.py`.
