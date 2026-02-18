ALTER TABLE chats ADD COLUMN is_starred BOOLEAN NOT NULL DEFAULT FALSE;
CREATE INDEX idx_chats_starred ON chats(user_id, is_starred) WHERE is_starred = TRUE;

ALTER TABLE chat_messages ADD COLUMN content_text TEXT;

-- ParadeDB does not support CHAR(n) columns for BM25 indexes
ALTER TABLE chats ALTER COLUMN id TYPE VARCHAR(26);
ALTER TABLE chat_messages ALTER COLUMN id TYPE VARCHAR(26);

-- BM25 index on chat titles for search (ngram for partial matching on short titles)
CREATE INDEX chat_title_search_idx ON chats
USING bm25 (
    id,
    (title::pdb.ngram(2, 3))
)
WITH (key_field = 'id');

-- BM25 index on chat message content for search (default unicode tokenizer)
CREATE INDEX chat_message_content_search_idx ON chat_messages
USING bm25 (
    id,
    content_text
)
WITH (key_field = 'id');
