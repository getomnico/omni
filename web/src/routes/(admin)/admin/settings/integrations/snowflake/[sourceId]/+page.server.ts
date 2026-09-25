import { error, redirect, fail } from '@sveltejs/kit'
import type { Actions, PageServerLoad } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { getOAuthManifestForSourceType } from '$lib/server/oauth/connectorOAuth'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { AuthType, SourceType } from '$lib/types'

function objectConfig(value: unknown): Record<string, unknown> {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) return {}
    return value as Record<string, unknown>
}

function parseHttpsUrl(value: string, label: string): URL {
    let parsed: URL
    try {
        parsed = new URL(value)
    } catch {
        throw new Error(`${label} must be a valid HTTPS URL`)
    }
    if (
        parsed.protocol !== 'https:' ||
        parsed.username ||
        parsed.password ||
        (parsed.port && parsed.port !== '443') ||
        parsed.search ||
        parsed.hash
    ) {
        throw new Error(`${label} must be a credential-free HTTPS URL`)
    }
    return parsed
}

function parseList(value: string, label: string): string[] {
    const entries = value
        .split(',')
        .map((entry) => entry.trim())
        .filter(Boolean)
    if (entries.some((entry) => entry.length > 255))
        throw new Error(`${label} entries are too long`)
    return [...new Set(entries)]
}

function parseBoolean(form: FormData, name: string): boolean {
    const value = form.get(name)
    if (value === null) return false
    if (value === 'true' || value === 'on') return true
    if (value === 'false') return false
    throw new Error(`Invalid ${name} setting`)
}

async function snowflakeSource(sourceId: string) {
    const source = await getSourceById(sourceId)
    if (!source) throw error(404, 'Source not found')
    if (source.sourceType !== SourceType.SNOWFLAKE)
        throw error(400, 'Invalid source type for this page')
    return source
}

export const load: PageServerLoad = async ({ params, locals }) => {
    const { user } = requireAdmin(locals)
    const source = await snowflakeSource(params.sourceId)
    const config = objectConfig(source.config)
    const [manifest, credentials, orgCredentials, userCredentials] = await Promise.all([
        getOAuthManifestForSourceType(SourceType.SNOWFLAKE),
        serviceCredentialsRepository.getByUserAndSource(source.id, user.id),
        serviceCredentialsRepository.getOrgCredsBySourceId(source.id),
        serviceCredentialsRepository.listUserCredentialsForSource(source.id),
    ])
    const credentialConfig = objectConfig(credentials?.config)
    const grantedScopes = Array.isArray(credentialConfig.granted_scopes)
        ? credentialConfig.granted_scopes.filter(
              (scope): scope is string => typeof scope === 'string',
          )
        : []

    return {
        source: { id: source.id, name: source.name, isActive: source.isActive },
        config: {
            accountUrl: typeof config.account_url === 'string' ? config.account_url : '',
            warehouse: typeof config.warehouse === 'string' ? config.warehouse : '',
            role: typeof config.role === 'string' ? config.role : '',
            databases: Array.isArray(config.databases)
                ? config.databases.filter((v): v is string => typeof v === 'string')
                : [],
            schemasAllowlist: Array.isArray(config.schemas_allowlist)
                ? config.schemas_allowlist.filter((v): v is string => typeof v === 'string')
                : [],
            schemasDenylist: Array.isArray(config.schemas_denylist)
                ? config.schemas_denylist.filter((v): v is string => typeof v === 'string')
                : [],
            includedObjectTypes: Array.isArray(config.included_object_types)
                ? config.included_object_types.filter((v): v is string => typeof v === 'string')
                : [],
            syncEnabled: config.sync_enabled !== false,
            includeTags: config.include_tags === true,
            mcpEnabled: config.mcp_enabled === true,
            mcpEndpointUrl:
                typeof config.mcp_endpoint_url === 'string' ? config.mcp_endpoint_url : '',
            canChangeMcpEndpoint: !config.source_binding && userCredentials.length === 0,
            hasOrgJwtCredentials: orgCredentials?.authType === AuthType.JWT,
            writeToolsEnabled: config.write_tools_enabled === true,
            readOnly: config.read_only !== false,
        },
        actionAuth: {
            authorized: Boolean(credentials),
            principalEmail: credentials?.principalEmail ?? null,
            grantedScopes,
        },
        oauth: {
            registrationRequiresInitialAccessToken:
                manifest?.registration_requires_initial_access_token === true,
        },
    }
}

export const actions: Actions = {
    default: async ({ request, params, locals }) => {
        if (!locals.user || locals.user.role !== 'admin') throw error(403, 'Admin access required')
        const source = await snowflakeSource(params.sourceId)
        const current = objectConfig(source.config)
        const form = await request.formData()

        try {
            const enabled = parseBoolean(form, 'enabled')
            const accountText = String(form.get('accountUrl') ?? '').trim()
            const accountUrl = parseHttpsUrl(accountText, 'Snowflake account URL')
            const storedAccountUrl =
                typeof current.account_url === 'string' ? current.account_url : null
            if (storedAccountUrl && accountUrl.origin !== new URL(storedAccountUrl).origin) {
                throw new Error(
                    'The Snowflake account cannot be changed here because its credentials and user authorizations are bound to the existing account. Create a new source to connect a different account.',
                )
            }
            if (accountUrl.pathname !== '/' || accountUrl.hostname.length > 253) {
                throw new Error('Snowflake account URL must contain only the account origin')
            }
            const syncEnabled = parseBoolean(form, 'syncEnabled')
            const mcpEnabled = parseBoolean(form, 'mcpEnabled')
            if (syncEnabled && current.sync_enabled === false) {
                const orgCredentials = await serviceCredentialsRepository.getOrgCredsBySourceId(
                    source.id,
                )
                if (orgCredentials?.authType !== AuthType.JWT) {
                    throw new Error(
                        'Metadata sync requires organization Snowflake JWT credentials. Credential setup or rotation is not available on this page; create a new source with metadata credentials to enable sync.',
                    )
                }
            }
            const warehouse = String(form.get('warehouse') ?? '').trim()
            const role = String(form.get('role') ?? '').trim()
            const databases = parseList(String(form.get('databases') ?? ''), 'Database')
            if (syncEnabled && (!warehouse || !role || databases.length === 0)) {
                throw new Error(
                    'Warehouse, role, and at least one database are required when metadata sync is enabled',
                )
            }
            const endpointText = String(form.get('mcpEndpointUrl') ?? '').trim()
            let mcpEndpointUrl: string | null = null
            if (endpointText) {
                const endpoint = parseHttpsUrl(endpointText, 'Managed MCP endpoint')
                if (
                    endpoint.origin !== accountUrl.origin ||
                    !/^\/api\/v2\/databases\/[^/]+\/schemas\/[^/]+\/mcp-servers\/[^/]+$/.test(
                        endpoint.pathname,
                    )
                ) {
                    throw new Error(
                        'Managed MCP endpoint must use the configured account and managed MCP path',
                    )
                }
                mcpEndpointUrl = endpoint.href.replace(/\/$/, '')
                const storedEndpoint =
                    typeof current.mcp_endpoint_url === 'string'
                        ? current.mcp_endpoint_url.replace(/\/$/, '')
                        : null
                if (
                    storedEndpoint &&
                    storedEndpoint !== mcpEndpointUrl &&
                    (current.source_binding ||
                        (await serviceCredentialsRepository.listUserCredentialsForSource(source.id))
                            .length > 0)
                ) {
                    throw new Error(
                        'The managed MCP endpoint cannot be changed while account-bound authorization exists. Create a new source to use a different endpoint.',
                    )
                }
            }
            if (mcpEnabled && !mcpEndpointUrl)
                throw new Error('Managed MCP endpoint is required when MCP is enabled')
            const schemasAllowlist = parseList(
                String(form.get('schemasAllowlist') ?? ''),
                'Schema allowlist',
            )
            const schemasDenylist = parseList(
                String(form.get('schemasDenylist') ?? ''),
                'Schema denylist',
            )
            if (schemasAllowlist.some((schema) => schemasDenylist.includes(schema))) {
                throw new Error('A schema cannot appear in both the allowlist and denylist')
            }
            const includedObjectTypes = parseList(
                String(form.get('includedObjectTypes') ?? ''),
                'Object type',
            )
            const config = {
                ...current,
                account_url: accountUrl.origin,
                warehouse: syncEnabled ? warehouse : warehouse || null,
                role: syncEnabled ? role : role || null,
                databases: syncEnabled ? databases : databases,
                schemas_allowlist: schemasAllowlist.length ? schemasAllowlist : null,
                schemas_denylist: schemasDenylist.length ? schemasDenylist : null,
                included_object_types: includedObjectTypes.length
                    ? includedObjectTypes
                    : current.included_object_types,
                sync_enabled: syncEnabled,
                include_tags: parseBoolean(form, 'includeTags'),
                mcp_enabled: mcpEnabled,
                mcp_endpoint_url: mcpEndpointUrl,
                oauth_issuer_url: accountUrl.origin,
                write_tools_enabled: mcpEnabled && parseBoolean(form, 'writeToolsEnabled'),
                read_only: !mcpEnabled || parseBoolean(form, 'readOnly'),
            }
            await updateSourceById(source.id, { isActive: enabled, config })

            if (enabled && syncEnabled) {
                const connectorManagerUrl = getConfig().services.connectorManagerUrl
                try {
                    await fetch(`${connectorManagerUrl}/sync/${source.id}`, {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                    })
                } catch (err) {
                    console.error(`Failed to trigger Snowflake sync for source ${source.id}:`, err)
                }
            }
        } catch (err) {
            if (err instanceof Error && !(err as { status?: number }).status) {
                return fail(400, { message: err.message })
            }
            throw err
        }
        throw redirect(303, '/admin/settings/integrations')
    },
}
