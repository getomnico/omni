-- 082_create_evaluation_tables.sql
-- Create core RAG evaluation telemetry tables.

-- Evaluation traces: one row per evaluated RAG interaction
CREATE TABLE IF NOT EXISTS eval_traces (
    id              TEXT PRIMARY KEY,
    query           TEXT NOT NULL,
    task_family     TEXT,                    -- 'current_state', 'timeline_change', etc. (for future extension)
    temporal_type   TEXT,                    -- 'current', 'historical', 'evolution', etc. (for future extension)
    
    -- Retrieval stage
    retrieved_doc_ids   TEXT[],              -- IDs of retrieved documents
    retrieved_scores    REAL[],              -- Retrieval scores
    retrieval_views     TEXT[],              -- 'fulltext', 'semantic', 'hybrid'
    fts_result_count    INTEGER,
    semantic_result_count INTEGER,
    retrieval_latency_ms INTEGER,
    
    -- Context assembly stage (Extensible JSONB for chunks)
    context_chunks      JSONB,              -- [{doc_id, chunk_index, start_offset, end_offset, score}]
    context_token_count INTEGER,
    context_truncated   BOOLEAN DEFAULT FALSE,
    chunk_duplication_rate REAL,             -- 0.0 to 1.0 (for future use)
    
    -- Generation stage
    generated_answer    TEXT,
    citations           JSONB,              -- [{doc_title, doc_url, cited_text}]
    generation_tokens   INTEGER,
    generation_latency_ms INTEGER,
    
    -- Source metadata
    source_types        TEXT[],             -- Source types in retrieved results
    languages           TEXT[],             -- Detected languages
    
    -- Eval metadata
    golden_set_id       TEXT,               -- NULL for production samples
    is_production       BOOLEAN DEFAULT FALSE,
    user_id             TEXT,
    chat_id             TEXT,
    
    created_at          TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Evaluation scores: one row per (trace, metric) pair
CREATE TABLE IF NOT EXISTS eval_scores (
    id              TEXT PRIMARY KEY,
    trace_id        TEXT NOT NULL REFERENCES eval_traces(id) ON DELETE CASCADE,
    metric_name     TEXT NOT NULL,           -- 'faithfulness', 'context_recall', etc.
    metric_category TEXT NOT NULL,           -- 'retrieval', 'generation', 'temporal', 'operational'
    score           REAL NOT NULL,           -- Normalized 0.0 to 1.0
    raw_score       REAL,                    -- Original scale before normalization
    reasoning       TEXT,                    -- LLM judge explanation
    judge_model     TEXT,                    -- Which model judged this
    
    created_at      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    
    UNIQUE(trace_id, metric_name)
);

-- Evaluation runs: one row per batch evaluation execution
CREATE TABLE IF NOT EXISTS eval_runs (
    id              TEXT PRIMARY KEY,
    run_type        TEXT NOT NULL,            -- 'golden_set', 'production_sample', 'ci_gate'
    dataset_version TEXT,                     -- Git hash or version tag of golden set
    config          JSONB,                    -- EvalConfig snapshot
    
    -- Aggregate results
    total_traces    INTEGER NOT NULL DEFAULT 0,
    metrics_summary JSONB,                    -- {metric_name: {mean, p50, p95, min, max}}
    
    -- Classification
    pass            BOOLEAN,                  -- CI gate pass/fail
    fail_reasons    TEXT[],                   -- Which thresholds were violated
    
    started_at      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at    TIMESTAMPTZ,
    
    created_at      TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for efficient querying
CREATE INDEX IF NOT EXISTS idx_eval_traces_task_family ON eval_traces(task_family);
CREATE INDEX IF NOT EXISTS idx_eval_traces_created_at ON eval_traces(created_at);
CREATE INDEX IF NOT EXISTS idx_eval_traces_golden_set ON eval_traces(golden_set_id) WHERE golden_set_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_eval_scores_metric ON eval_scores(metric_name, metric_category);
CREATE INDEX IF NOT EXISTS idx_eval_scores_trace ON eval_scores(trace_id);
CREATE INDEX IF NOT EXISTS idx_eval_runs_type ON eval_runs(run_type);
