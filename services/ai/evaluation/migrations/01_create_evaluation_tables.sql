-- Create core RAG evaluation tables.

-- Evaluation scores: one row per (trace, metric) pair
CREATE TABLE IF NOT EXISTS eval_scores (
    id              TEXT PRIMARY KEY,
    trace_id        TEXT,
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
CREATE INDEX IF NOT EXISTS idx_eval_scores_metric ON eval_scores(metric_name, metric_category);
CREATE INDEX IF NOT EXISTS idx_eval_scores_trace ON eval_scores(trace_id);
CREATE INDEX IF NOT EXISTS idx_eval_runs_type ON eval_runs(run_type);
