-- Add authoritative source data to canonical people records.
--
-- `people` remains one row per human, merged by normalized business email.
-- `source_data` is an object keyed by source ID. Each value contains that
-- source's current, reviewed workplace-directory data for the person:
--
-- {
--   "<source_id>": {
--     "external_id": "EMP-123",
--     "display_name": "Ada Lovelace",
--     "department": "Engineering",
--     "manager_external_id": "EMP-100",
--     "source_updated_at": "2025-01-01T00:00:00Z"
--   }
-- }
--
-- A missing source key means that source no longer reports the person. This
-- permits source-scoped deletion without a second person table.
-- Only reviewed workplace fields are stored; raw provider data is excluded.

BEGIN;

ALTER TABLE people
    ADD COLUMN IF NOT EXISTS middle_name VARCHAR(128),
    ADD COLUMN IF NOT EXISTS work_country VARCHAR(128),
    ADD COLUMN IF NOT EXISTS employment_start_date DATE,
    ADD COLUMN IF NOT EXISTS employment_end_date DATE,
    ADD COLUMN IF NOT EXISTS grade VARCHAR(64),
    ADD COLUMN IF NOT EXISTS band VARCHAR(64),
    ADD COLUMN IF NOT EXISTS confirmation_status VARCHAR(64),
    ADD COLUMN IF NOT EXISTS source_data JSONB NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE people
    ADD CONSTRAINT people_source_data_object
    CHECK (jsonb_typeof(source_data) = 'object');

-- Existing people may differ only by email case. Merge those rows before
-- enforcing the normalized identity used by all person-producing sources.
CREATE TEMP TABLE people_email_dedupe ON COMMIT DROP AS
WITH ranked AS (
    SELECT id,
           first_value(id) OVER (
               PARTITION BY lower(trim(email))
               ORDER BY char_length(COALESCE(display_name, '')) DESC, id ASC
           ) AS keeper_id,
           count(*) OVER (PARTITION BY lower(trim(email))) AS duplicate_count
    FROM people
)
SELECT id AS duplicate_id, keeper_id
FROM ranked
WHERE duplicate_count > 1 AND id <> keeper_id;

-- Merge duplicate rows in stable ID order. Existing keeper values win; the
-- first non-null duplicate fills missing scalar fields. JSON objects are
-- merged with the same precedence, activity is preserved if any row is active,
-- and audit timestamps retain the full observed range.
DO $$
DECLARE
    duplicate RECORD;
BEGIN
    FOR duplicate IN
        SELECT d.duplicate_id, d.keeper_id
        FROM people_email_dedupe d
        ORDER BY d.keeper_id, d.duplicate_id
    LOOP
        -- Only pre-existing (pre-108) columns can differ here: the columns
        -- added above are born empty within this same transaction, so merging
        -- them would be a no-op.
        UPDATE people keeper
        SET display_name = COALESCE(keeper.display_name, loser.display_name),
            given_name = COALESCE(keeper.given_name, loser.given_name),
            surname = COALESCE(keeper.surname, loser.surname),
            avatar_url = COALESCE(keeper.avatar_url, loser.avatar_url),
            job_title = COALESCE(keeper.job_title, loser.job_title),
            department = COALESCE(keeper.department, loser.department),
            division = COALESCE(keeper.division, loser.division),
            company_name = COALESCE(keeper.company_name, loser.company_name),
            office_location = COALESCE(keeper.office_location, loser.office_location),
            manager_id = COALESCE(keeper.manager_id, loser.manager_id),
            city = COALESCE(keeper.city, loser.city),
            state = COALESCE(keeper.state, loser.state),
            country = COALESCE(keeper.country, loser.country),
            employee_id = COALESCE(keeper.employee_id, loser.employee_id),
            employee_type = COALESCE(keeper.employee_type, loser.employee_type),
            cost_center = COALESCE(keeper.cost_center, loser.cost_center),
            is_active = keeper.is_active OR loser.is_active,
            metadata = loser.metadata || keeper.metadata,
            external_id = COALESCE(keeper.external_id, loser.external_id),
            created_at = LEAST(keeper.created_at, loser.created_at),
            updated_at = GREATEST(keeper.updated_at, loser.updated_at)
        FROM people loser
        WHERE keeper.id = duplicate.keeper_id
          AND loser.id = duplicate.duplicate_id;
    END LOOP;
END;
$$;

UPDATE people p
SET manager_id = d.keeper_id
FROM people_email_dedupe d
WHERE p.manager_id = d.duplicate_id;

-- A duplicate that pointed at its canonical keeper (or vice versa) must not
-- turn into a self-manager reference after ID redirection.
UPDATE people SET manager_id = NULL WHERE manager_id = id;

DELETE FROM people p
USING people_email_dedupe d
WHERE p.id = d.duplicate_id;

UPDATE people SET email = lower(trim(email));

CREATE UNIQUE INDEX IF NOT EXISTS idx_people_email_lower_unique
    ON people (lower(email));
CREATE INDEX IF NOT EXISTS idx_people_source_data
    ON people USING gin (source_data);

-- Dedicated Person mutation dequeue preserves FIFO per source/email across
-- sync-run types and retries. This expression index keeps predecessor checks
-- scoped to the affected identity rather than scanning the whole queue.
CREATE INDEX IF NOT EXISTS idx_connector_events_person_identity_order
    ON connector_events_queue (
        source_id,
        lower(btrim(CASE event_type
            WHEN 'person_sync' THEN payload #>> '{person,email}'
            ELSE payload ->> 'email'
        END)),
        id
    )
    WHERE event_type IN ('person_sync', 'person_deleted')
      AND status IN ('pending', 'processing', 'failed');

DROP INDEX people_search_idx;
CREATE INDEX people_search_idx ON people
USING bm25 (
    id,
    (email::pdb.simple('ascii_folding=true')),
    (display_name::pdb.simple('ascii_folding=true')),
    (given_name::pdb.simple('ascii_folding=true')),
    (middle_name::pdb.simple('ascii_folding=true')),
    (surname::pdb.simple('ascii_folding=true')),
    (department::pdb.simple('ascii_folding=true')),
    (division::pdb.simple('ascii_folding=true')),
    (job_title::pdb.simple('ascii_folding=true')),
    (company_name::pdb.simple('ascii_folding=true')),
    (office_location::pdb.simple('ascii_folding=true')),
    (employee_id::pdb.simple('ascii_folding=true')),
    (employee_type::pdb.simple('ascii_folding=true')),
    (cost_center::pdb.simple('ascii_folding=true')),
    (work_country::pdb.simple('ascii_folding=true')),
    (grade::pdb.simple('ascii_folding=true')),
    (band::pdb.simple('ascii_folding=true')),
    (confirmation_status::pdb.simple('ascii_folding=true'))
)
WITH (key_field = 'id');

-- Source cleanup enqueues internal person-deletion runs with a dedicated
-- trigger type; admit it in the sync_runs trigger_type whitelist.
ALTER TABLE sync_runs DROP CONSTRAINT IF EXISTS sync_runs_trigger_type_check;
ALTER TABLE sync_runs ADD CONSTRAINT sync_runs_trigger_type_check
CHECK (trigger_type IN ('scheduled', 'manual', 'webhook', 'source_cleanup'));

COMMIT;
