ALTER TABLE tool_approvals
    ADD COLUMN IF NOT EXISTS tool_call_id TEXT;

UPDATE tool_approvals ta
SET tool_call_id = (
    SELECT block->>'id'
    FROM chat_messages cm
    CROSS JOIN LATERAL jsonb_array_elements(cm.message::jsonb->'content') AS block
    WHERE cm.chat_id = ta.chat_id
      AND cm.created_at <= ta.created_at
      AND cm.message::jsonb->>'role' = 'assistant'
      AND block->>'type' = 'tool_use'
      AND block->>'name' = ta.tool_name
      AND block->'input' = ta.tool_input
    ORDER BY cm.message_seq_num DESC
    LIMIT 1
)
WHERE ta.tool_call_id IS NULL;
