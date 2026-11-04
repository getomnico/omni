import { beforeEach, describe, expect, it, vi } from 'vitest'

const state = vi.hoisted(() => {
    const value = {
        source: null as Record<string, unknown> | null,
        credentials: [] as Record<string, unknown>[],
        oauthStates: new Map<string, Record<string, unknown>>(),
        ready: new Map<string, (userId: string) => Promise<string>>(),
        binding: null as Record<string, string> | null,
        credentialId: 0,
        sourcesTable: { table: 'sources' },
        credentialsTable: {
            table: 'service_credentials',
            userId: Symbol('user_id'),
            sourceId: Symbol('source_id'),
            id: Symbol('id'),
        },
        transaction: vi.fn(),
    }
    value.transaction = vi.fn(
        async (callback: (tx: Record<string, (...args: never[]) => unknown>) => unknown) => {
            const tx = {
                select: (projection?: Record<string, unknown>) => selectQuery(projection),
                update: () => ({
                    set: (values: Record<string, unknown>) => ({
                        where: async () => {
                            value.source = { ...value.source, ...values }
                        },
                    }),
                }),
                delete: () => ({
                    where: async (condition: unknown) => {
                        const userId = conditionValue(condition, value.credentialsTable.userId)
                        value.credentials = value.credentials.filter(
                            (credential) => credential.userId !== userId,
                        )
                    },
                }),
                insert: (table: unknown) => ({
                    values: async (values: Record<string, unknown>) => {
                        if (table === value.credentialsTable) value.credentials.push(values)
                    },
                }),
            }
            return callback(tx)
        },
    )
    return value
})

function conditionValue(condition: unknown, column: symbol): unknown {
    if (!Array.isArray(condition)) return undefined
    if (condition.length === 2 && condition[0] === column) return condition[1]
    for (const item of condition) {
        const found = conditionValue(item, column)
        if (found !== undefined) return found
    }
    return undefined
}

function selectQuery(projection?: Record<string, unknown>) {
    let table: unknown
    let userId: unknown
    const query = {
        from(value: unknown) {
            table = value
            return query
        },
        where(condition: unknown) {
            userId = conditionValue(condition, state.credentialsTable.userId)
            return query
        },
        for() {
            return Promise.resolve(rows())
        },
        then(resolve: (value: unknown[]) => unknown, reject: (reason: unknown) => unknown) {
            return Promise.resolve(rows()).then(resolve, reject)
        },
    }
    function rows(): unknown[] {
        if (table === state.sourcesTable) return state.source ? [state.source] : []
        if (table === state.credentialsTable) {
            const matching = state.credentials.filter((credential) => credential.userId === userId)
            return projection ? matching.map(({ id }) => ({ id })) : matching
        }
        return []
    }
    return query
}

vi.mock('$lib/server/db', () => ({
    db: {
        transaction: state.transaction,
        update: () => ({ set: () => ({ where: vi.fn() }) }),
    },
}))
vi.mock('$lib/server/db/schema', () => ({
    sources: state.sourcesTable,
    serviceCredentials: state.credentialsTable,
}))
vi.mock('drizzle-orm', () => ({
    and: (...args: unknown[]) => args,
    eq: (...args: unknown[]) => args,
    isNull: (...args: unknown[]) => args,
}))
vi.mock('ulid', () => ({ ulid: () => `credential-${++state.credentialId}` }))
vi.mock('$lib/server/oauth/connectorOAuth', () => ({
    exchangeCodeAndIdentify: async (_code: string, token: string) => state.oauthStates.get(token),
    oauthCredentialExpiry: () => null,
    requestOAuthCredentialValidation: async () => state.binding,
    SOURCE_BINDING_CONFIG_KEY: 'oauth_source_binding',
    sourceBindingPolicyApplies: () => true,
}))
vi.mock('$lib/server/oauth/state', () => ({ OAuthStateManager: { getState: vi.fn() } }))
vi.mock('$lib/server/repositories/service-credentials', () => ({
    serviceCredentialsRepository: {
        getByUserAndSource: async (sourceId: string, userId: string) =>
            state.credentials.find(
                (credential) => credential.sourceId === sourceId && credential.userId === userId,
            ) ?? null,
        getOrgCredsBySourceId: vi.fn(),
        createForUser: vi.fn(),
    },
}))
vi.mock('$lib/server/crypto/encryption', () => ({
    decryptConfig: (value: unknown) => value,
    encryptConfig: (value: unknown) => value,
}))
vi.mock('$lib/server/logger', () => ({ logger: { error: vi.fn(), warn: vi.fn() } }))
vi.mock('$lib/server/config', () => ({
    getConfig: () => ({ services: { connectorManagerUrl: 'http://connector-manager' } }),
}))
vi.mock('$lib/server/db/sources', () => ({
    getSourceById: async () => state.source,
    getSourcesByType: vi.fn(),
}))
vi.mock('$lib/server/db/tool-approvals', () => ({
    toolApprovalRepository: { approvePendingOAuth: vi.fn() },
}))
vi.mock('$lib/utils/icons', () => ({ getSourceDisplayName: () => 'CRM' }))

import { GET } from '../../../routes/api/oauth/callback/+server'

const bindingPolicy = {
    allow_user_establish: true,
    initial_admin_required: true,
    authenticated_discovery_required: true,
}

function makeState(userId: string, token: string, scopes: string[] = []) {
    state.oauthStates.set(token, {
        tokens: { access_token: `access-${userId}`, scope: scopes.join(' ') },
        state: {
            user_id: userId,
            metadata: {
                requiredScopes: scopes,
                flow: { type: 'user_read', sourceId: 'source-1' },
            },
        },
        principalEmail: `${userId}@example.com`,
        config: {
            provider: 'salesforce',
            credential_provider: 'salesforce',
            token_endpoint: 'https://auth.example/token',
            source_binding_policy: bindingPolicy,
            token_response_fields: [],
        },
        clientCreds: { clientId: 'client', tokenEndpoint: 'https://auth.example/token' },
        userinfo: {},
    })
}

function makeSource(config: Record<string, unknown> = {}) {
    state.source = {
        id: 'source-1',
        isDeleted: false,
        isActive: true,
        updatedAt: new Date('2026-01-01T00:00:00.000Z'),
        config: { sync_enabled: false, ...config },
        integrationType: 'salesforce',
        scope: 'org',
    }
}

function callback(userId: string, token: string, ready: (userId: string) => Promise<string>) {
    state.ready.set(userId, ready)
    return Promise.resolve(
        GET({
            url: new URL(`http://localhost/api/oauth/callback?code=code&state=${token}`),
            locals: { user: { id: userId, email: `${userId}@example.com`, role: 'admin' } },
            fetch: async (_input: RequestInfo | URL, init?: RequestInit) => {
                const body = JSON.parse(String(init?.body)) as { user_id: string }
                return Response.json({
                    status: await state.ready.get(body.user_id)?.(body.user_id),
                })
            },
        } as never),
    )
}

async function redirectOf(promise: Promise<unknown>) {
    try {
        await promise
    } catch (err) {
        return err as { status: number; location: string }
    }
    throw new Error('Expected OAuth callback to redirect')
}

function deferred<T>() {
    let resolve!: (value: T) => void
    const promise = new Promise<T>((done) => (resolve = done))
    return { promise, resolve }
}

beforeEach(() => {
    state.credentials = []
    state.oauthStates.clear()
    state.ready.clear()
    state.binding = { organization_id: 'org-1' }
    state.credentialId = 0
    makeSource()
    state.transaction.mockClear()
})

describe('OAuth callback source binding persistence', () => {
    it('stores a first admin binding with the credential and activates only after discovery', async () => {
        makeState('admin-1', 'first')
        const result = await redirectOf(callback('admin-1', 'first', async () => 'completed'))

        expect(result.status).toBe(302)
        expect(state.source?.config).toMatchObject({
            oauth_source_binding: { organization_id: 'org-1' },
        })
        expect(state.credentials).toHaveLength(1)
        expect(state.source?.isActive).toBe(true)
        expect(state.transaction).toHaveBeenCalledTimes(2)
    })

    it('rejects first-time binding by a non-admin without persisting credentials', async () => {
        makeState('member-1', 'nonadmin')
        const memberCallback = Promise.resolve(
            GET({
                url: new URL('http://localhost/api/oauth/callback?code=code&state=nonadmin'),
                locals: { user: { id: 'member-1', email: 'member@example.com', role: 'member' } },
                fetch: async () => Response.json({ status: 'completed' }),
            } as never),
        )
        const result = await redirectOf(memberCallback)

        expect(result.location).toContain('oauth/done?ok=false')
        expect(state.credentials).toHaveLength(0)
        expect(
            (state.source?.config as Record<string, unknown>).oauth_source_binding,
        ).toBeUndefined()
    })

    it('allows reauthorization for the same binding and rejects a cross-organization identity', async () => {
        makeSource({ oauth_source_binding: { organization_id: 'org-1' } })
        makeState('admin-1', 'reauth')
        await redirectOf(callback('admin-1', 'reauth', async () => 'completed'))
        expect(state.credentials).toHaveLength(1)

        state.credentials = []
        state.binding = { organization_id: 'org-2' }
        makeState('admin-2', 'cross-org')
        const result = await redirectOf(callback('admin-2', 'cross-org', async () => 'completed'))
        expect(result.location).toContain('oauth/done?ok=false')
        expect(state.credentials).toHaveLength(0)
        expect((state.source?.config as Record<string, unknown>).oauth_source_binding).toEqual({
            organization_id: 'org-1',
        })
    })

    it('keeps the source inactive when authenticated discovery fails', async () => {
        makeState('admin-1', 'discovery-fails')
        await redirectOf(callback('admin-1', 'discovery-fails', async () => 'failed'))

        expect(state.credentials).toHaveLength(1)
        expect(state.source?.isActive).toBe(false)
    })

    it('does not let an earlier user callback reactivate after a later callback fails discovery', async () => {
        makeState('user-a', 'callback-a')
        makeState('user-b', 'callback-b')
        const discoveryStarted = deferred<void>()
        const releaseDiscovery = deferred<string>()
        const callbackA = callback('user-a', 'callback-a', async () => {
            discoveryStarted.resolve()
            return releaseDiscovery.promise
        })

        await discoveryStarted.promise
        const generationA = (state.source?.updatedAt as Date).getTime()
        await redirectOf(callback('user-b', 'callback-b', async () => 'failed'))
        expect((state.source?.updatedAt as Date).getTime()).toBeGreaterThan(generationA)
        releaseDiscovery.resolve('completed')
        const staleResult = await redirectOf(callbackA)

        expect(staleResult.location).toContain('oauth/done?ok=false')
        expect(state.credentials.map((credential) => credential.userId)).toEqual([
            'user-a',
            'user-b',
        ])
        expect(state.source?.isActive).toBe(false)
    })
})
