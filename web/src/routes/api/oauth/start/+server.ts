import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getSourceById } from '$lib/server/db/sources'
import {
    generateAuthUrl,
    generateAuthUrlForOrgSource,
    generateAuthUrlForUserWrite,
    isProviderConfigured,
    getOAuthManifestForSourceType,
} from '$lib/server/oauth/connectorOAuth'

function oauthClientNotConfiguredMessage(provider: string): string {
    return (
        `OAuth client for ${provider} is not configured. Ask an admin to set it up under ` +
        'Admin → Settings → Integrations → OAuth Apps.'
    )
}

/// Unified OAuth start route. Flows are disambiguated by query params:
///   ?source_types=google_drive,gmail          → connect_source flow
///   ?source_id=01J...&flow=org_source         → admin org-source credential flow
///   ?source_id=01J...&flow=user_write         → per-user action credential flow
/// Optional `return_to` is preserved through the callback for UI return links.
export const GET: RequestHandler = async ({ url, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const sourceId = url.searchParams.get('source_id')
    const flow = url.searchParams.get('flow') ?? 'user_write'
    const sourceTypesParam = url.searchParams.get('source_types')
    const returnTo = url.searchParams.get('return_to') ?? undefined

    if (sourceId) {
        if (flow !== 'org_source' && flow !== 'user_write') {
            throw error(400, 'flow must be either org_source or user_write')
        }

        const source = await getSourceById(sourceId)
        if (!source || source.isDeleted) throw error(404, 'Source not found')
        if (source.scope === 'user') {
            if (flow === 'org_source') {
                throw error(400, 'org_source OAuth is only valid for org-wide sources')
            }
            if (source.createdBy !== locals.user.id) {
                throw error(403, "Cannot attach OAuth credentials to another user's source")
            }
        } else if (source.scope !== 'org') {
            throw error(400, `Unsupported source scope: ${source.scope}`)
        }

        const config = await getOAuthManifestForSourceType(source.sourceType)
        if (!config) {
            throw error(501, `OAuth is not implemented for source_type=${source.sourceType} yet.`)
        }
        if (!(await isProviderConfigured(config.provider, config))) {
            throw error(412, oauthClientNotConfiguredMessage(config.provider))
        }

        if (flow === 'org_source') {
            if (locals.user.role !== 'admin') {
                throw error(403, 'Admin access required')
            }
            const { url: authUrl } = await generateAuthUrlForOrgSource({
                sourceId,
                sourceType: source.sourceType,
                userId: locals.user.id,
                returnTo,
            })
            throw redirect(302, authUrl)
        }

        const { url: authUrl } = await generateAuthUrlForUserWrite({
            sourceId,
            sourceType: source.sourceType,
            userId: locals.user.id,
            returnTo,
        })
        throw redirect(302, authUrl)
    }

    if (sourceTypesParam) {
        const sourceTypes = sourceTypesParam.split(',').filter(Boolean)
        if (sourceTypes.length === 0) throw error(400, 'source_types must not be empty')

        const config = await getOAuthManifestForSourceType(sourceTypes[0])
        if (!config) {
            throw error(501, `OAuth not implemented for source_type=${sourceTypes[0]}`)
        }
        if (!(await isProviderConfigured(config.provider, config))) {
            throw error(412, oauthClientNotConfiguredMessage(config.provider))
        }

        const { url: authUrl } = await generateAuthUrl({
            flow: { type: 'connect_source', sourceTypes, returnTo },
            userId: locals.user.id,
        })
        throw redirect(302, authUrl)
    }

    throw error(400, 'Either source_id or source_types must be provided')
}
