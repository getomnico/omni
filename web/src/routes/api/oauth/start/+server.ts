import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getSourceById } from '$lib/server/db/sources'
import {
    generateAuthUrl,
    generateAuthUrlForUserWrite,
    isProviderConfigured,
    getOAuthManifestForSourceType,
} from '$lib/server/oauth/connectorOAuth'

/// Unified OAuth start route. Two flows, disambiguated by query params:
///   ?source_types=google_drive,gmail          → connect_source flow
///   ?source_id=01J...                         → user_write flow
/// Optional `return_to` is preserved through the callback for UI return links.
export const GET: RequestHandler = async ({ url, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const sourceId = url.searchParams.get('source_id')
    const sourceTypesParam = url.searchParams.get('source_types')
    const returnTo = url.searchParams.get('return_to') ?? undefined

    if (sourceId) {
        const source = await getSourceById(sourceId)
        if (!source || source.isDeleted) throw error(404, 'Source not found')
        if (source.scope !== 'org') {
            throw error(
                400,
                'Per-user OAuth attaches to org-wide sources only. Personal sources already use the owner credential.',
            )
        }

        const config = await getOAuthManifestForSourceType(source.sourceType)
        if (!config) {
            throw error(
                501,
                `Per-user OAuth is not implemented for source_type=${source.sourceType} yet.`,
            )
        }
        if (!(await isProviderConfigured(config.provider))) {
            throw error(
                412,
                `OAuth client for ${config.provider} is not configured. Ask an admin to set it up under Admin → Settings → Integrations.`,
            )
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
        if (!(await isProviderConfigured(config.provider))) {
            throw error(
                412,
                `OAuth client for ${config.provider} is not configured. Ask an admin to set it up under Admin → Settings → Integrations.`,
            )
        }

        const { url: authUrl } = await generateAuthUrl({
            flow: { type: 'connect_source', sourceTypes, returnTo },
            userId: locals.user.id,
        })
        throw redirect(302, authUrl)
    }

    throw error(400, 'Either source_id or source_types must be provided')
}
