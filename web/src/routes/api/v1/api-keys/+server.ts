import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { createApiKey, listApiKeys, revokeApiKey, deleteApiKey } from '$lib/server/apiKeys'

function parseJson(request: Request): Promise<Record<string, unknown>> {
    return request.json().catch(() => null) as Promise<Record<string, unknown>>
}

function isValidUlid(id: unknown): id is string {
    return typeof id === 'string' && /^[0-9A-HJKMNP-TV-Z]{26}$/i.test(id)
}

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    const keys = await listApiKeys(locals.user.id)
    return json({ keys })
}

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    if (locals.user.mustChangePassword) {
        return json({ error: 'Password change required before creating API keys' }, { status: 403 })
    }

    const body = await parseJson(request)
    if (!body) {
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    const { name, expires_at } = body

    if (!name || typeof name !== 'string' || name.trim().length === 0 || name.trim().length > 255) {
        return json({ error: 'Name is required (max 255 characters)' }, { status: 400 })
    }

    const expiresAt = expires_at ? new Date(expires_at as string) : undefined
    if (expiresAt && isNaN(expiresAt.getTime())) {
        return json({ error: 'Invalid expires_at date' }, { status: 400 })
    }
    if (expiresAt && expiresAt <= new Date()) {
        return json({ error: 'expires_at must be in the future' }, { status: 400 })
    }

    try {
        const result = await createApiKey(locals.user.id, name.trim(), expiresAt)
        return json(
            {
                id: result.id,
                key: result.key,
                prefix: result.prefix,
                message: 'Store this key securely — it will not be shown again.',
            },
            { status: 201 },
        )
    } catch (error) {
        const msg = error instanceof Error ? error.message : 'Failed to create API key'
        return json({ error: msg }, { status: 400 })
    }
}

export const PATCH: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    const body = await parseJson(request)
    if (!body) {
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    const { id, action } = body

    if (!isValidUlid(id)) {
        return json({ error: 'Invalid API key id' }, { status: 400 })
    }

    if (action === 'revoke') {
        const revoked = await revokeApiKey(id, locals.user.id)
        if (!revoked) {
            return json({ error: 'API key not found' }, { status: 404 })
        }
        return json({ success: true })
    }

    return json({ error: 'Invalid action' }, { status: 400 })
}

export const DELETE: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    const body = await parseJson(request)
    if (!body) {
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    const { id } = body

    if (!isValidUlid(id)) {
        return json({ error: 'Invalid API key id' }, { status: 400 })
    }

    const deleted = await deleteApiKey(id, locals.user.id)
    if (!deleted) {
        return json({ error: 'API key not found' }, { status: 404 })
    }

    return json({ success: true })
}
