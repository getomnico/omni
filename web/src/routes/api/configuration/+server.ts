import { error, json } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { deleteUser, getGlobal, getUser, setGlobal, setUser } from '$lib/server/db/configuration'

type Scope = 'global' | 'user'

interface ReadBody {
    scope: Scope
    key: string
}

interface WriteBody extends ReadBody {
    value: Record<string, unknown> | null
}

function parseBody(raw: unknown): WriteBody | null {
    if (!raw || typeof raw !== 'object') return null
    const { scope, key, value } = raw as Partial<WriteBody>
    if (scope !== 'global' && scope !== 'user') return null
    if (typeof key !== 'string' || key.length === 0) return null
    if (value !== null && (typeof value !== 'object' || Array.isArray(value))) return null
    return { scope, key, value: value ?? null }
}

function authorize(scope: Scope, locals: App.Locals): { userId: string } {
    if (!locals.user) throw error(401, 'Unauthorized')
    if (scope === 'global') {
        if (locals.user.role !== 'admin') throw error(403, 'Admin access required')
    }
    return { userId: locals.user.id }
}

export const GET: RequestHandler = async ({ url, locals }) => {
    const scope = url.searchParams.get('scope') as Scope | null
    const key = url.searchParams.get('key')
    if ((scope !== 'global' && scope !== 'user') || !key) {
        throw error(400, 'scope must be "global" or "user" and key is required')
    }
    const { userId } = authorize(scope, locals)
    const value = scope === 'global' ? await getGlobal(key) : await getUser(userId, key)
    return json({ scope, key, value })
}

export const PATCH: RequestHandler = async ({ request, locals }) => {
    const body = parseBody(await request.json().catch(() => null))
    if (!body) {
        throw error(400, 'Body must be { scope: "global"|"user", key: string, value: object|null }')
    }
    const { userId } = authorize(body.scope, locals)
    if (body.scope === 'global') {
        if (body.value === null) {
            throw error(400, 'Cannot clear a global configuration (value: null)')
        }
        await setGlobal(body.key, body.value)
    } else if (body.value === null) {
        await deleteUser(userId, body.key)
    } else {
        await setUser(userId, body.key, body.value)
    }
    return json({ ok: true, scope: body.scope, key: body.key, value: body.value })
}
