import { beforeEach, describe, expect, it, vi } from 'vitest'
import { load, actions } from './+page.server'

const mocks = vi.hoisted(() => ({
    getSourceById: vi.fn(),
    updateSourceById: vi.fn(),
    getOAuthManifestForSourceType: vi.fn(),
    getByUserAndSource: vi.fn(),
    requireAdmin: vi.fn(),
    fetch: vi.fn(),
}))

vi.mock('$lib/server/authHelpers', () => ({ requireAdmin: mocks.requireAdmin }))
vi.mock('$lib/server/db/sources', () => ({
    getSourceById: mocks.getSourceById,
    updateSourceById: mocks.updateSourceById,
}))
vi.mock('$lib/server/config', () => ({
    getConfig: () => ({ services: { connectorManagerUrl: 'http://connector-manager' } }),
}))
vi.mock('$lib/server/oauth/connectorOAuth', () => ({
    getOAuthManifestForSourceType: mocks.getOAuthManifestForSourceType,
}))
vi.mock('$lib/server/repositories/service-credentials', () => ({
    serviceCredentialsRepository: { getByUserAndSource: mocks.getByUserAndSource },
}))

const baseSource = {
    id: 'source-1',
    name: 'Snowflake prod',
    sourceType: 'snowflake',
    isActive: true,
    config: {
        account_url: 'https://acme.snowflakecomputing.com',
        warehouse: 'META',
        role: 'READER',
        databases: ['ANALYTICS'],
        sync_enabled: true,
        mcp_enabled: true,
        mcp_endpoint_url:
            'https://acme.snowflakecomputing.com/api/v2/databases/ANALYTICS/schemas/PUBLIC/mcp-servers/omni',
        private_key: 'must-not-be-returned',
        connector_binding: { internal: 'preserve-me' },
    },
}

function event(form: FormData) {
    return {
        locals: { user: { id: 'admin-1', role: 'admin' } },
        params: { sourceId: 'source-1' },
        request: { formData: async () => form },
    } as never
}

beforeEach(() => {
    vi.clearAllMocks()
    mocks.getSourceById.mockResolvedValue(structuredClone(baseSource))
    mocks.requireAdmin.mockReturnValue({ user: { id: 'admin-1', role: 'admin' } })
    mocks.getOAuthManifestForSourceType.mockResolvedValue(null)
    mocks.getByUserAndSource.mockResolvedValue(null)
    vi.stubGlobal('fetch', mocks.fetch)
    mocks.fetch.mockResolvedValue({ ok: true })
})

describe('Snowflake settings route', () => {
    it('returns only safe display configuration, never connector secrets or internal bindings', async () => {
        const result = await load(event(new FormData()))
        const pageData = result as { config: { accountUrl: string } }
        expect(pageData.config.accountUrl).toBe('https://acme.snowflakecomputing.com')
        expect(JSON.stringify(pageData)).not.toContain('must-not-be-returned')
        expect(JSON.stringify(pageData)).not.toContain('preserve-me')
    })

    it('rejects an account endpoint mismatch and does not persist it', async () => {
        const form = new FormData()
        form.set('enabled', 'true')
        form.set('syncEnabled', 'true')
        form.set('mcpEnabled', 'true')
        form.set('accountUrl', 'https://acme.snowflakecomputing.com')
        form.set('warehouse', 'META')
        form.set('role', 'READER')
        form.set('databases', 'ANALYTICS')
        form.set(
            'mcpEndpointUrl',
            'https://attacker.example/api/v2/databases/ANALYTICS/schemas/PUBLIC/mcp-servers/omni',
        )
        const result = await actions.default?.(event(form))
        expect(result).toMatchObject({ status: 400 })
        expect(mocks.updateSourceById).not.toHaveBeenCalled()
    })

    it('preserves unknown connector config and never syncs an MCP-only source', async () => {
        const form = new FormData()
        form.set('enabled', 'true')
        form.set('syncEnabled', 'false')
        form.set('mcpEnabled', 'true')
        form.set('accountUrl', 'https://acme.snowflakecomputing.com')
        form.set('warehouse', 'META')
        form.set('role', 'READER')
        form.set('databases', 'ANALYTICS')
        form.set(
            'mcpEndpointUrl',
            'https://acme.snowflakecomputing.com/api/v2/databases/ANALYTICS/schemas/PUBLIC/mcp-servers/omni',
        )
        form.set('includeTags', 'false')
        form.set('writeToolsEnabled', 'false')
        form.set('readOnly', 'true')
        await expect(actions.default?.(event(form))).rejects.toMatchObject({ status: 303 })
        const saved = mocks.updateSourceById.mock.calls[0]?.[1]
        expect(saved.config.private_key).toBe('must-not-be-returned')
        expect(saved.config.connector_binding).toEqual({ internal: 'preserve-me' })
        expect(mocks.fetch).not.toHaveBeenCalled()
    })
})
