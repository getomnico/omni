-- Rewrite IMAP thread external_ids to remove the embedded source_id segment.
--
-- Old format: imap-thread:{source_id}:{folder}:{thread_id}
--   source_id is always a 26-char ULID (alphanumeric, no colons)
-- New format: imap-thread:{folder}:{thread_id}
--
-- The regex captures the prefix "imap-thread:", skips the row's own source_id
-- followed by a colon, and keeps the rest (folder:thread_id).
--
-- The WHERE clause limits the update to rows whose external_id actually
-- contains their own source_id (guaranteeing old-format; avoids false matches
-- on new-format rows whose folder happens to be 26 alphanumeric chars).
-- A CTE is used so that ON CONFLICT (source_id, external_id) collisions
-- (possible if new-format rows were already inserted by the updated connector)
-- are handled gracefully: conflicting rows are deleted in favour of the
-- already-existing new-format row.

WITH to_rewrite AS (
    SELECT id, source_id,
           external_id AS old_external_id,
           'imap-thread:' || substring(external_id FROM char_length('imap-thread:' || source_id || ':') + 1) AS new_external_id
    FROM documents
    WHERE external_id LIKE 'imap-thread:%'
      AND external_id LIKE 'imap-thread:' || source_id || ':%'
),
-- Detect rows whose new_external_id would collide within the same source_id
collisions AS (
    SELECT tr.id
    FROM to_rewrite tr
    JOIN documents d ON d.source_id = tr.source_id
                    AND d.external_id = tr.new_external_id
                    AND d.id != tr.id
),
-- Delete old-format rows that would collide (the new-format row is already correct)
deleted AS (
    DELETE FROM documents
    WHERE id IN (SELECT id FROM collisions)
    RETURNING id
)
-- Update the remaining old-format rows
UPDATE documents d
SET external_id = tr.new_external_id
FROM to_rewrite tr
WHERE d.id = tr.id
  AND d.id NOT IN (SELECT id FROM collisions);
