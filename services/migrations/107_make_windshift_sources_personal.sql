-- Windshift uses delegated per-user OAuth. Convert the original organization
-- source model to one personal source per Omni user.

-- If the source owner already completed a per-user action authorization, keep
-- that credential (it includes the read scopes needed by sync) and remove the
-- older org credential.
DELETE FROM service_credentials org_credential
USING sources source
WHERE org_credential.source_id = source.id
  AND org_credential.user_id IS NULL
  AND source.source_type = 'windshift'
  AND source.scope = 'org'
  AND EXISTS (
      SELECT 1
      FROM service_credentials owner_credential
      WHERE owner_credential.source_id = source.id
        AND owner_credential.user_id = source.created_by
  );

-- Otherwise move the original sync credential to the source owner.
UPDATE service_credentials credential
SET user_id = source.created_by,
    updated_at = NOW()
FROM sources source
WHERE credential.source_id = source.id
  AND credential.user_id IS NULL
  AND source.source_type = 'windshift'
  AND source.scope = 'org';

-- Credentials other users attached to the former org source must not remain
-- usable after it becomes the owner's personal source.
DELETE FROM service_credentials credential
USING sources source
WHERE credential.source_id = source.id
  AND credential.user_id IS NOT NULL
  AND credential.user_id <> source.created_by
  AND source.source_type = 'windshift'
  AND source.scope = 'org';

UPDATE sources
SET scope = 'user',
    updated_at = NOW()
WHERE source_type = 'windshift'
  AND scope = 'org';

COMMENT ON COLUMN service_credentials.user_id IS
    'For personal sources, identifies the source owner credential; for org sources, identifies a user action credential; NULL identifies an org credential.';

-- Existing indexed documents must become private immediately; a later sync
-- will continue emitting the same owner-only ACL.
UPDATE documents document
SET permissions = jsonb_build_object(
        'public', false,
        'users', jsonb_build_array(lower(owner_user.email)),
        'groups', '[]'::jsonb
    ),
    updated_at = NOW()
FROM sources source
JOIN users owner_user ON owner_user.id = source.created_by
WHERE document.source_id = source.id
  AND source.source_type = 'windshift';
