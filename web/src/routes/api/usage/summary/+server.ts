import { json } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { RequestHandler } from './$types.js'

export const GET: RequestHandler = async ({ url, locals }) => {
    if (!locals.user || locals.user.role !== 'admin') {
        return json({ error: 'Admin access required' }, { status: 403 })
    }

    const days = url.searchParams.get('days') ?? '30'
    const userId = url.searchParams.get('user_id') ?? ''

    const params = new URLSearchParams({ days })
    if (userId) params.set('user_id', userId)

    const response = await fetch(`${env.AI_SERVICE_URL}/usage/summary?${params.toString()}`)

    if (!response.ok) {
        return json({ error: 'Failed to fetch usage summary' }, { status: response.status })
    }

    return json(await response.json())
}
