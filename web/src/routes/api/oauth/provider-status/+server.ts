import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { isProviderConfigured } from '$lib/server/oauth/connectorOAuth'

/// Lightweight check used by chat OAuth-required cards to know whether the
/// admin has wired up an OAuth client/secret for a given provider. Returns
/// 401 if the requester isn't signed in (chat-scoped UI only).
export const GET: RequestHandler = async ({ url, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }
    const provider = url.searchParams.get('provider')
    if (!provider) {
        throw error(400, 'provider query param is required')
    }
    return json({ configured: await isProviderConfigured(provider) })
}
