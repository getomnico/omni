import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'

interface GoogleConnectorUser {
    id: string
    email: string
    name: string
    org_unit: string
    suspended: boolean
    is_admin: boolean
}

interface GoogleConnectorResponse {
    users: GoogleConnectorUser[]
    next_page_token?: string
    has_more: boolean
}

export const GET: RequestHandler = async ({ url, locals, fetch }) => {
    try {
        // Check authentication
        const session = locals.session
        const user = locals.user
        if (!user) {
            throw error(401, 'Unauthorized')
        }

        // Check if user is admin
        if (user.role !== 'admin') {
            throw error(403, 'Forbidden - Admin access required')
        }

        // Get query parameters
        const query = url.searchParams.get('q') || ''
        const limit = Math.min(parseInt(url.searchParams.get('limit') || '50'), 100)
        const pageToken = url.searchParams.get('pageToken') || ''
        const sourceId = url.searchParams.get('sourceId')

        if (!sourceId) {
            throw error(400, 'sourceId parameter is required')
        }

        const googleConnectorUrl = process.env.GOOGLE_CONNECTOR_URL

        // Build the search URL for the Google connector
        const searchParams = new URLSearchParams()
        if (query) searchParams.set('q', query)
        searchParams.set('limit', limit.toString())
        if (pageToken) searchParams.set('page_token', pageToken)

        const connectorUrl = `${googleConnectorUrl}/users/search/${sourceId}?${searchParams}`

        // Call the Google connector service
        const response = await fetch(connectorUrl, {
            method: 'GET',
            headers: {
                'Content-Type': 'application/json',
            },
        })

        if (!response.ok) {
            console.error('Google connector error:', response.status, response.statusText)

            if (response.status === 401) {
                throw error(401, 'Google service credentials invalid or expired')
            } else if (response.status === 400) {
                throw error(400, 'Invalid source ID or missing Google integration settings')
            } else if (response.status === 404) {
                throw error(404, 'Google integration not found for this source')
            }

            throw error(500, 'Failed to search users in Google Workspace')
        }

        const connectorData: GoogleConnectorResponse = await response.json()

        // Transform the response for the frontend
        const users = connectorData.users.map((user) => ({
            id: user.id,
            email: user.email,
            name: user.name,
            orgUnit: user.org_unit,
            suspended: user.suspended,
            isAdmin: user.is_admin,
        }))

        return json({
            users,
            nextPageToken: connectorData.next_page_token,
            hasMore: connectorData.has_more,
        })
    } catch (err) {
        console.error('Error searching Google users:', err)

        // Re-throw SvelteKit errors
        if (err && typeof err === 'object' && 'status' in err) {
            throw err
        }

        throw error(500, 'Failed to search users')
    }
}
