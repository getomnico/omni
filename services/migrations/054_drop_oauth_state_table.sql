-- OAuth state is now stored in Redis with automatic TTL expiry
DROP FUNCTION IF EXISTS cleanup_expired_oauth_state();
DROP TABLE IF EXISTS oauth_state;
