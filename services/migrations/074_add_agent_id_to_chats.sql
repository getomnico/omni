ALTER TABLE chats ADD COLUMN agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL;
CREATE INDEX idx_chats_agent_id ON chats(agent_id) WHERE agent_id IS NOT NULL;
