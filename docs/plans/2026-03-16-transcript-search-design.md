# Transcript Search & RAG Design

## Overview

Add an embedding-based search system to deskmic so users (and agents) can query
their transcript history. Two new CLI subcommands: `deskmic index` builds and
maintains a local vector index, `deskmic search` retrieves relevant conversation
chunks.

## CLI Interface

### `deskmic index`

Idempotent indexing. Scans all transcript `.jsonl` files, chunks utterances into
conversation segments, embeds them via Azure OpenAI, and stores everything in a
local SQLite database. On rerun, only processes transcripts that have changed
since the last index.

Example output:

```
Indexed 47 new chunks from 3 transcripts (245 total chunks)
```

### `deskmic search "<query>"`

Embeds the query, runs vector similarity search, returns top N results.

Flags:
- `--from DATE` / `--to DATE` -- filter by date range
- `--source mic|teams` -- filter by audio source
- `--limit N` -- number of results (default 10)
- `--json` -- output as JSON for agent consumption

Default output:

```
[2026-03-16 09:37-09:42] (mic, score: 0.84)
Here we go. How are you guys? I don't mind...

[2026-03-05 14:12-14:18] (teams, score: 0.79)
So for the deployment pipeline, we decided to...
```

JSON mode outputs an array of objects:

```json
[{
  "date": "2026-03-16",
  "start_time": "09:37",
  "end_time": "09:42",
  "source": "mic",
  "score": 0.84,
  "text": "...",
  "files": ["mic_09-37-31.wav", "mic_09-37-43.wav"]
}]
```

## Chunking Pipeline

The core insight: individual utterances (5-12 seconds, 1-2 sentences) are too
short for meaningful retrieval. We group them into conversation-level chunks
using gap-based splitting.

For each transcript `.jsonl` file:

1. **Load utterances** -- parse each line as
   `{timestamp, source, duration_secs, file, text}`.
2. **Sort by filename** -- filenames encode time (`mic_09-37-31.wav`), so
   lexicographic sort gives chronological order.
3. **Gap-based grouping** -- walk utterances sequentially. Start a new chunk
   when:
   - Gap between end of previous utterance and start of next exceeds
     **60 seconds** (configurable).
   - Current chunk exceeds **5 minutes** of audio duration (configurable).
   - Source changes (`mic` to `teams` or vice versa).
4. **Build chunk text** -- concatenate all utterance texts with a single space.
   Store metadata: date, source, start time, end time, constituent filenames,
   total duration.
5. **Chunk ID** -- deterministic hash of `(date, source, start_file)`. If the
   chunk ID already exists in the DB, skip it. This is what makes indexing
   idempotent.

## SQLite Schema

Single database file at `<output_directory>/deskmic-search.db`.

```sql
CREATE TABLE indexed_files (
    file_name TEXT PRIMARY KEY,
    modified_at INTEGER NOT NULL,
    indexed_at INTEGER NOT NULL
);

CREATE TABLE chunks (
    id TEXT PRIMARY KEY,
    date TEXT NOT NULL,
    source TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT NOT NULL,
    duration_secs REAL NOT NULL,
    text TEXT NOT NULL,
    files TEXT NOT NULL,
    embedding BLOB NOT NULL
);

CREATE INDEX idx_chunks_date ON chunks(date);
CREATE INDEX idx_chunks_source ON chunks(source);
```

### Idempotency

On each `deskmic index` run, compare each `.jsonl` file's mtime against
`indexed_files.modified_at`. If unchanged, skip. If changed or new, delete all
chunks for that date, re-chunk, re-embed, and re-insert. This handles the case
where transcription appends new utterances to an existing file.

### Vector Search

The `sqlite-vec` extension provides vector similarity search via a virtual
table, joined against the chunks table for metadata filtering.

## Embedding & Search Flow

### Indexing

1. Scan transcript directory, compare mtimes against `indexed_files` table.
2. For changed/new files: run the chunking pipeline.
3. Batch embed via Azure OpenAI
   `POST /openai/deployments/text-embedding-3-large/embeddings`. The API
   accepts up to 16 inputs per request; batch accordingly.
4. Insert chunks and embeddings in a single SQLite transaction.
5. Update `indexed_files` with new mtime.

### Searching

1. Embed the query string (single input, same endpoint).
2. Use `sqlite-vec` to find top K nearest vectors by cosine similarity.
3. Apply SQL WHERE filters (date range, source) -- pre-filter if sqlite-vec
   supports it, otherwise retrieve K*3 candidates and filter in application
   code.
4. Format and print results.

## Configuration

New `[search]` section in `deskmic.toml`:

```toml
[search]
embedding_deployment = "text-embedding-3-large"
chunk_gap_secs = 60
chunk_max_duration_secs = 300
```

Reuses the existing `[transcription.azure]` endpoint and API key. No new
credentials needed.

## Dependencies

- `rusqlite` (bundled feature) -- SQLite with no system dependency.
- `sqlite-vec` -- vector search extension loaded at runtime into rusqlite.
- Existing `reqwest` -- reused for Azure OpenAI embedding calls.

## Module Structure

- `src/search/mod.rs` -- public API, CLI wiring.
- `src/search/chunker.rs` -- gap-based chunking (pure functions, testable).
- `src/search/embeddings.rs` -- Azure OpenAI embeddings client.
- `src/search/db.rs` -- SQLite schema, chunk CRUD, sqlite-vec integration.
- `src/search/indexer.rs` -- full index pipeline orchestration.

## Testing

- **Chunker**: unit tests with synthetic utterances (gap splitting, max
  duration cap, source boundaries).
- **DB**: integration tests with in-memory SQLite (insert, query, idempotency,
  filtering).
- **Embeddings**: unit tests with mocked HTTP responses.
- **Indexer**: integration test with temp directory and mock embeddings.

## Out of Scope

- Incremental/streaming indexing (full reindex per changed file is fine at this
  scale of ~2 MB of transcripts).
- Local embeddings model (Azure only, matching existing project pattern).
- Hybrid search (keyword + vector).
- Reranking step.
