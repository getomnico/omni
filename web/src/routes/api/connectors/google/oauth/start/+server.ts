import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { GoogleConnectorOAuthService } from '$lib/server/oauth/googleConnector'

export const GET: RequestHandler = async ({ locals, url }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const serviceTypesParam = url.searchParams.get('serviceTypes')

    if (!serviceTypesParam) {
        throw error(400, 'Missing serviceTypes parameter')
    }

    const serviceTypes = serviceTypesParam.split(',').filter(Boolean)
    const validTypes = ['google_drive', 'gmail']

    for (const type of serviceTypes) {
        if (!validTypes.includes(type)) {
            throw error(400, `Invalid service type: ${type}`)
        }
    }

    if (serviceTypes.length === 0) {
        throw error(400, 'At least one service type is required')
    }

    const authUrl = await GoogleConnectorOAuthService.generateAuthUrl(serviceTypes, locals.user.id)

    throw redirect(302, authUrl)
}
