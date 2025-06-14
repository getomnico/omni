import { pgTable, text, timestamp, boolean, jsonb, bigint, integer } from 'drizzle-orm/pg-core'

export const user = pgTable('users', {
    id: text('id').primaryKey(),
    email: text('email').notNull().unique(),
    passwordHash: text('password_hash').notNull(),
    role: text('role').notNull().default('user'),
    isActive: boolean('is_active').notNull().default(true),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const sources = pgTable('sources', {
    id: text('id').primaryKey(),
    name: text('name').notNull(),
    sourceType: text('source_type').notNull(),
    config: jsonb('config').notNull().default({}),
    isActive: boolean('is_active').notNull().default(true),
    lastSyncAt: timestamp('last_sync_at', { withTimezone: true, mode: 'date' }),
    syncStatus: text('sync_status').default('pending'),
    syncError: text('sync_error'),
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
    modelName: text('model_name').notNull().default('intfloat/e5-large-v2'),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export const oauthCredentials = pgTable('oauth_credentials', {
    id: text('id').primaryKey(),
    sourceId: text('source_id')
        .notNull()
        .references(() => sources.id, { onDelete: 'cascade' }),
    provider: text('provider').notNull(),
    clientId: text('client_id'),
    clientSecret: text('client_secret'),
    accessToken: text('access_token'),
    refreshToken: text('refresh_token'),
    tokenType: text('token_type'),
    expiresAt: timestamp('expires_at', { withTimezone: true, mode: 'date' }),
    metadata: jsonb('metadata').notNull().default({}),
    createdAt: timestamp('created_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
    updatedAt: timestamp('updated_at', { withTimezone: true, mode: 'date' }).notNull().defaultNow(),
})

export type User = typeof user.$inferSelect
export type Source = typeof sources.$inferSelect
export type Document = typeof documents.$inferSelect
export type Embedding = typeof embeddings.$inferSelect
export type OAuthCredentials = typeof oauthCredentials.$inferSelect
