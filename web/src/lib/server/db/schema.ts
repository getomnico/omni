import {
    pgTable,
    text,
    timestamp,
    boolean,
    jsonb,
    bigint,
    integer,
    real,
    pgEnum,
    check,
} from 'drizzle-orm/pg-core'
import type { MessageParam } from '@anthropic-ai/sdk/resources/messages.js'
import { sql } from 'drizzle-orm'

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

export const sources = pgTable('sources', {
    id: text('id').primaryKey(),
    name: text('name').notNull(),
    sourceType: text('source_type').notNull(),
    config: jsonb('config').notNull().default({}),
    isActive: boolean('is_active').notNull().default(true),
    isDeleted: boolean('is_deleted').notNull().default(false),
    lastSyncAt: timestamp('last_sync_at', { withTimezone: true, mode: 'date' }),
    syncStatus: text('sync_status').default('pending'),
    syncError: text('sync_error'),
    userFilterMode: text('user_filter_mode').notNull().default('all'),
    userWhitelist: jsonb('user_whitelist').notNull().default('[]'),
    userBlacklist: jsonb('user_blacklist').notNull().default('[]'),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    createdBy: text('created_by')
        .notNull()
        .references(() => user.id),
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

export const chats = pgTable('chats', {
    id: text('id').primaryKey(),
    userId: text('user_id')
        .notNull()
        .references(() => user.id, { onDelete: 'cascade' }),
    title: text('title'),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const chatMessages = pgTable('chat_messages', {
    id: text('id').primaryKey(),
    chatId: text('chat_id')
        .notNull()
        .references(() => chats.id, { onDelete: 'cascade' }),
    messageSeqNum: integer('message_seq_num').notNull(),
    message: jsonb('message').$type<MessageParam>().notNull(),
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

export const configuration = pgTable('configuration', {
    key: text('key').primaryKey(),
    value: jsonb('value').notNull(),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export type User = typeof user.$inferSelect
export type Source = typeof sources.$inferSelect
export type Document = typeof documents.$inferSelect
export type Embedding = typeof embeddings.$inferSelect
export type ServiceCredentials = typeof serviceCredentials.$inferSelect
export type ConnectorEventsQueue = typeof connectorEventsQueue.$inferSelect
export type SyncRun = typeof syncRuns.$inferSelect
export type ApprovedDomain = typeof approvedDomains.$inferSelect
export type MagicLink = typeof magicLinks.$inferSelect
export type Chat = typeof chats.$inferSelect
export type ChatMessage = typeof chatMessages.$inferSelect
export type ResponseFeedback = typeof responseFeedback.$inferSelect
export type Configuration = typeof configuration.$inferSelect
