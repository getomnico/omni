import { pgTable, text, timestamp, boolean, jsonb, bigint, integer, primaryKey } from 'drizzle-orm/pg-core'
import type { MessageParam } from '@anthropic-ai/sdk/resources/messages.js'

export const user = pgTable('users', {
    id: text('id').primaryKey(),
    email: text('email').notNull().unique(),
    passwordHash: text('password_hash'),
    role: text('role').notNull().default('user'),
    isActive: boolean('is_active').notNull().default(true),
    authMethod: text('auth_method').notNull().default('password'),
    domain: text('domain'),
    mustChangePassword: boolean('must_change_password').notNull().default(false),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const userPreferences = pgTable(
    'user_preferences',
    {
        userId: text('user_id')
            .notNull()
            .references(() => user.id, { onDelete: 'cascade' }),
        key: text('key').notNull(),
        value: jsonb('value').notNull(),
        createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
        updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    },
    (table) => ({ pk: primaryKey({ columns: [table.userId, table.key] }) }),
)

export const sources = pgTable('sources', {
    id: text('id').primaryKey(),
    name: text('name').notNull(),
    sourceType: text('source_type').notNull(),
    config: jsonb('config').notNull().default({}),
    isActive: boolean('is_active').notNull().default(true),
    isDeleted: boolean('is_deleted').notNull().default(false),
    userFilterMode: text('user_filter_mode').notNull().default('all'),
    userWhitelist: jsonb('user_whitelist').notNull().default('[]'),
    userBlacklist: jsonb('user_blacklist').notNull().default('[]'),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    createdBy: text('created_by')
        .notNull()
        .references(() => user.id),
    syncIntervalSeconds: integer('sync_interval_seconds'),
})

export const documents = pgTable('documents', {
    id: text('id').primaryKey(),
    sourceId: text('source_id')
        .notNull()
        .references(() => sources.id, { onDelete: 'cascade' }),
    externalId: text('external_id').notNull(),
    title: text('title').notNull(),
    content: text('content'),
    contentType: text('content_type'),
    fileSize: bigint('file_size', { mode: 'number' }),
    fileExtension: text('file_extension'),
    url: text('url'),
    parentId: text('parent_id'),
    metadata: jsonb('metadata').notNull().default({}),
    permissions: jsonb('permissions').notNull().default([]),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    lastIndexedAt: timestamp('last_indexed_at', { withTimezone: true, mode: 'date' })
        .notNull()
        .defaultNow(),
})

export const embeddings = pgTable('embeddings', {
    id: text('id').primaryKey(),
    documentId: text('document_id')
        .notNull()
        .references(() => documents.id, { onDelete: 'cascade' }),
    chunkIndex: integer('chunk_index').notNull(),
    chunkText: text('chunk_text').notNull(),
    modelName: text('model_name').notNull(),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const serviceCredentials = pgTable('service_credentials', {
    id: text('id').primaryKey(),
    sourceId: text('source_id')
        .notNull()
        .references(() => sources.id, { onDelete: 'cascade' }),
    provider: text('provider').notNull(),
    authType: text('auth_type').notNull(),
    principalEmail: text('principal_email'),
    credentials: jsonb('credentials').notNull(),
    config: jsonb('config').notNull().default({}),
    expiresAt: timestamp('expires_at', { withTimezone: true, mode: 'date' }),
    lastValidatedAt: timestamp('last_validated_at', { withTimezone: true, mode: 'date' }),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const connectorEventsQueue = pgTable('connector_events_queue', {
    id: text('id').primaryKey(),
    sourceId: text('source_id').notNull(),
    eventType: text('event_type').notNull(),
    payload: jsonb('payload').notNull(),
    status: text('status').notNull().default('pending'),
    retryCount: integer('retry_count').default(0),
    maxRetries: integer('max_retries').default(3),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    processedAt: timestamp('processed_at', { withTimezone: true, mode: 'date' }),
    errorMessage: text('error_message'),
})

export const syncRuns = pgTable('sync_runs', {
    id: text('id').primaryKey(),
    sourceId: text('source_id')
        .notNull()
        .references(() => sources.id, { onDelete: 'cascade' }),
    syncType: text('sync_type').notNull(),
    startedAt: timestamp('started_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    completedAt: timestamp('completed_at', { withTimezone: true, mode: 'date' }),
    status: text('status').notNull().default('running'),
    documentsScanned: integer('documents_scanned').default(0),
    documentsProcessed: integer('documents_processed').default(0),
    documentsUpdated: integer('documents_updated').default(0),
    errorMessage: text('error_message'),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const approvedDomains = pgTable('approved_domains', {
    id: text('id').primaryKey(),
    domain: text('domain').notNull().unique(),
    approvedBy: text('approved_by')
        .notNull()
        .references(() => user.id),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const magicLinks = pgTable('magic_links', {
    id: text('id').primaryKey(),
    email: text('email').notNull(),
    tokenHash: text('token_hash').notNull().unique(),
    expiresAt: timestamp('expires_at', { withTimezone: true, mode: 'date' }).notNull(),
    usedAt: timestamp('used_at', { withTimezone: true, mode: 'date' }),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    userId: text('user_id').references(() => user.id),
})

export const modelProviders = pgTable('model_providers', {
    id: text('id').primaryKey(),
    name: text('name').notNull(),
    providerType: text('provider_type').notNull(),
    config: jsonb('config').notNull().default({}),
    isDeleted: boolean('is_deleted').notNull().default(false),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const models = pgTable('models', {
    id: text('id').primaryKey(),
    modelProviderId: text('model_provider_id')
        .notNull()
        .references(() => modelProviders.id, { onDelete: 'cascade' }),
    modelId: text('model_id').notNull(),
    displayName: text('display_name').notNull(),
    isDefault: boolean('is_default').notNull().default(false),
    isSecondary: boolean('is_secondary').notNull().default(false),
    isDeleted: boolean('is_deleted').notNull().default(false),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const chats = pgTable('chats', {
    id: text('id').primaryKey(),
    userId: text('user_id')
        .notNull()
        .references(() => user.id, { onDelete: 'cascade' }),
    title: text('title'),
    isStarred: boolean('is_starred').notNull().default(false),
    modelId: text('model_id').references(() => models.id, {
        onDelete: 'set null',
    }),
    agentId: text('agent_id').references(() => agents.id, { onDelete: 'set null' }),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const chatMessages = pgTable('chat_messages', {
    id: text('id').primaryKey(),
    chatId: text('chat_id')
        .notNull()
        .references(() => chats.id, { onDelete: 'cascade' }),
    parentId: text('parent_id'),
    messageSeqNum: integer('message_seq_num').notNull(),
    message: jsonb('message').$type<MessageParam>().notNull(),
    contentText: text('content_text'),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const responseFeedback = pgTable('response_feedback', {
    id: text('id').primaryKey(),
    messageId: text('message_id')
        .notNull()
        .references(() => chatMessages.id, { onDelete: 'cascade' }),
    userId: text('user_id')
        .notNull()
        .references(() => user.id, { onDelete: 'cascade' }),
    feedbackType: text('feedback_type').notNull(),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const authProviders = pgTable('auth_providers', {
    provider: text('provider').primaryKey(),
    enabled: boolean('enabled').notNull().default(false),
    config: jsonb('config').notNull().default({}),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedBy: text('updated_by').references(() => user.id),
})

export const connectorConfigs = pgTable('connector_configs', {
    provider: text('provider').primaryKey(),
    config: jsonb('config').notNull().default({}),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedBy: text('updated_by').references(() => user.id),
})

export const toolApprovals = pgTable('tool_approvals', {
    id: text('id').primaryKey(),
    chatId: text('chat_id')
        .notNull()
        .references(() => chats.id, { onDelete: 'cascade' }),
    userId: text('user_id')
        .notNull()
        .references(() => user.id, { onDelete: 'cascade' }),
    toolName: text('tool_name').notNull(),
    toolInput: jsonb('tool_input').notNull(),
    sourceId: text('source_id'),
    sourceType: text('source_type'),
    status: text('status').notNull().default('pending'),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    resolvedAt: timestamp('resolved_at', { withTimezone: true, mode: 'date' }),
    resolvedBy: text('resolved_by').references(() => user.id),
})

export const embeddingProviders = pgTable('embedding_providers', {
    id: text('id').primaryKey(),
    name: text('name').notNull(),
    providerType: text('provider_type').notNull(),
    config: jsonb('config').notNull().default({}),
    isCurrent: boolean('is_current').notNull().default(false),
    isDeleted: boolean('is_deleted').notNull().default(false),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const emailProviders = pgTable('email_providers', {
    id: text('id').primaryKey(),
    name: text('name').notNull(),
    providerType: text('provider_type').notNull(),
    config: jsonb('config').notNull().default({}),
    isCurrent: boolean('is_current').notNull().default(false),
    isDeleted: boolean('is_deleted').notNull().default(false),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const agents = pgTable('agents', {
    id: text('id').primaryKey(),
    userId: text('user_id')
        .notNull()
        .references(() => user.id, { onDelete: 'cascade' }),
    name: text('name').notNull(),
    instructions: text('instructions').notNull(),
    agentType: text('agent_type').notNull().default('user'),
    scheduleType: text('schedule_type').notNull(),
    scheduleValue: text('schedule_value').notNull(),
    modelId: text('model_id').references(() => models.id, { onDelete: 'set null' }),
    allowedSources: jsonb('allowed_sources').notNull().default([]),
    allowedActions: jsonb('allowed_actions').notNull().default([]),
    isEnabled: boolean('is_enabled').notNull().default(true),
    isDeleted: boolean('is_deleted').notNull().default(false),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const agentRuns = pgTable('agent_runs', {
    id: text('id').primaryKey(),
    agentId: text('agent_id')
        .notNull()
        .references(() => agents.id, { onDelete: 'cascade' }),
    status: text('status').notNull().default('pending'),
    startedAt: timestamp('started_at', { withTimezone: true, mode: 'date' }),
    completedAt: timestamp('completed_at', { withTimezone: true, mode: 'date' }),
    executionLog: jsonb('execution_log').notNull().default([]),
    summary: text('summary'),
    errorMessage: text('error_message'),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const apiKeys = pgTable('api_keys', {
    id: text('id').primaryKey(),
    userId: text('user_id')
        .notNull()
        .references(() => user.id, { onDelete: 'cascade' }),
    keyHash: text('key_hash').notNull().unique(),
    keyPrefix: text('key_prefix').notNull(),
    name: text('name').notNull(),
    lastUsedAt: timestamp('last_used_at', { withTimezone: true, mode: 'date' }),
    expiresAt: timestamp('expires_at', { withTimezone: true, mode: 'date' }),
    allowedSources: jsonb('allowed_sources').$type<string[] | null>().default(null),
    scope: text('scope').notNull().default('public'),
    isActive: boolean('is_active').notNull().default(true),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const configuration = pgTable('configuration', {
    key: text('key').primaryKey(),
    value: jsonb('value').notNull().$type<Record<string, unknown>>(),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export type Configuration = typeof configuration.$inferSelect

export type User = typeof user.$inferSelect
export type Source = typeof sources.$inferSelect
export type Document = typeof documents.$inferSelect
export type Embedding = typeof embeddings.$inferSelect
export type ServiceCredentials = typeof serviceCredentials.$inferSelect
export type ConnectorEventsQueue = typeof connectorEventsQueue.$inferSelect
export type SyncRun = typeof syncRuns.$inferSelect
export type ApprovedDomain = typeof approvedDomains.$inferSelect
export type MagicLink = typeof magicLinks.$inferSelect
export type ModelProvider = typeof modelProviders.$inferSelect
export type Model = typeof models.$inferSelect
export type Chat = typeof chats.$inferSelect
export type ChatMessage = typeof chatMessages.$inferSelect
export type ResponseFeedback = typeof responseFeedback.$inferSelect
export type AuthProvider = typeof authProviders.$inferSelect
export type ConnectorConfig = typeof connectorConfigs.$inferSelect
export type EmbeddingProvider = typeof embeddingProviders.$inferSelect
export type ToolApproval = typeof toolApprovals.$inferSelect
export type EmailProvider = typeof emailProviders.$inferSelect
export type Agent = typeof agents.$inferSelect
export type AgentRun = typeof agentRuns.$inferSelect
export type ApiKey = typeof apiKeys.$inferSelect
