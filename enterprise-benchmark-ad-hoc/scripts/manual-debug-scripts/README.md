# Benchmark Debugging Scripts

Reusable scripts for investigating benchmark failures, search quality, and retrieval behavior. All scripts run on the VPS and connect to the benchmark Postgres database.

## Prerequisites

All scripts assume:
- Postgres is running locally on port 5432 (omni_benchmark / omni_bench)
- TEI embedding server is reachable at `http://172.18.0.1:18091/embed`
- Searcher is reachable at `http://localhost:3001`

## Scripts

### manual_search.py
**Purpose:** Generic semantic search debugging tool. Takes a query string, gets embeddings from TEI, and runs direct SQL cosine-similarity search against the embeddings table.

**Usage:**
```bash
python3 manual_search.py "your query here"
```

**When to use:**
- Manually reproducing what the agent's search found
- Debugging why a specific document didn't rank
- Comparing semantic vs BM25 results for a query

---

### debug_semantic.py
**Purpose:** Systematic semantic search analysis for known gold documents. Given a question and gold doc ID(s), it queries embeddings and reports:
- Gold doc distance (lower = better)
- Top-10 overall semantic results with distances

**Usage:** Edit the `queries` dict at the bottom of the script with your test cases, then run:
```bash
python3 debug_semantic.py
```

**When to use:**
- Investigating why a gold document wasn't retrieved
- Comparing gold doc distance to top-ranked docs
- Diagnosing embedding model limitations

---

### analyze_chat.py
**Purpose:** Extract the actual search queries an agent made during a benchmark chat session. Connects to DB, finds the chat by question text, and prints all `search_documents` tool calls.

**Usage:**
```bash
python3 analyze_chat.py "question keyword or phrase"
```

**When to use:**
- Debugging what the agent actually searched for
- Comparing agent queries vs optimal queries
- Understanding agent query generation patterns

---

### bm25_search.py
**Purpose:** Generic BM25 (fulltext) search via direct SQL using ParadeDB's pg_search index.

**Usage:** Edit the `queries` list in the script, then:
```bash
python3 bm25_search.py
```

**When to use:**
- Comparing BM25 vs semantic search for the same query
- Testing exact term matching behavior
- Finding documents by rare/unique vocabulary

---

### bm25_search2.py
**Purpose:** BM25 search with gold document rank tracking. Same as bm25_search.py but checks if a known gold doc appears in the results.

**Usage:** Edit `gold` and `queries` variables, then:
```bash
python3 bm25_search2.py
```

**When to use:**
- Determining if BM25 can find a gold doc when semantic search fails
- Measuring BM25 recall for specific questions

---

### check_doc_terminology.py
**Purpose:** Test semantic search using terminology extracted from a known gold document (NOT from the question). Checks if searching with the doc's own vocabulary finds it.

**Usage:** Edit `gold` and `queries` (using doc terminology), then:
```bash
python3 check_doc_terminology.py
```

**When to use:**
- Testing whether semantic search can find a doc even with "perfect" queries
- Diagnosing if the issue is query quality or embedding quality
- Establishing upper bounds on semantic search performance

---

### check_doc_terminology_bm25.py
**Purpose:** Same as check_doc_terminology.py but for BM25 search.

**Usage:** Edit `gold` and `queries`, then:
```bash
python3 check_doc_terminology_bm25.py
```

**When to use:**
- Comparing BM25 vs semantic for doc-terminology queries
- Determining if BM25 is the dominant signal for certain question types

---

### test_any_semantic_query.py
**Purpose:** Brute-force test many different semantic queries to see if ANY query finds a gold document. Useful for disproving the hypothesis that "a better query would have found it."

**Usage:** Edit the `queries` list with increasingly specific terms, then:
```bash
python3 test_any_semantic_query.py
```

**When to use:**
- Proving that semantic search fundamentally cannot find a document
- Establishing the limits of the embedding model
- Justifying the need for re-rankers or larger result sets

## Common Workflow

1. **Identify a failure:** Use `analyze_results.py` to find zero-recall questions
2. **Check agent queries:** Use `analyze_chat.py` to see what the agent searched
3. **Test semantic search:** Use `debug_semantic.py` to see gold doc distance
4. **Test BM25 search:** Use `bm25_search2.py` to check BM25 performance
5. **Find the limit:** Use `test_any_semantic_query.py` to see if ANY query works
6. **Read the gold doc:** Use `manual_search.py` with the doc title/ID to fetch it
