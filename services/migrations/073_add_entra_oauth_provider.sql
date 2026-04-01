-- 'entra' is intentionally separate from 'microsoft'. 'microsoft' is used for
-- connector OAuth credentials (OneDrive, Outlook, SharePoint, Teams), while
-- 'entra' is used for SSO login credentials via Microsoft Entra ID.
ALTER TABLE user_oauth_credentials DROP CONSTRAINT IF EXISTS user_oauth_credentials_provider_check;
ALTER TABLE user_oauth_credentials ADD CONSTRAINT user_oauth_credentials_provider_check
CHECK (provider IN ('google', 'slack', 'atlassian', 'github', 'microsoft', 'okta', 'entra'));

COMMENT ON CONSTRAINT user_oauth_credentials_provider_check ON user_oauth_credentials IS
    'microsoft = connector credentials (OneDrive, Outlook, etc.), entra = SSO login credentials (Microsoft Entra ID)';
