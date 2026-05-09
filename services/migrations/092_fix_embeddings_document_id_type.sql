-- Fix the type of embeddings.document_id to match documents.id.
--
-- documents.id is VARCHAR(26), but embeddings.document_id is CHAR(26),
-- it was never updated.

ALTER TABLE embeddings
    DROP CONSTRAINT embeddings_document_id_fkey;

ALTER TABLE embeddings
    ALTER COLUMN document_id TYPE VARCHAR(26);

ALTER TABLE embeddings
    ADD CONSTRAINT embeddings_document_id_fkey
        FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE;
