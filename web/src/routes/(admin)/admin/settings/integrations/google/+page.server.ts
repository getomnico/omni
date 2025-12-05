import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    getGoogleSources,
    updateGoogleSources,
    getActiveGoogleSources,
} from '$lib/server/db/sources'
import type { UserFilterMode } from '$lib/server/db/sources'

export const load: PageServerLoad = async ({ params, url, locals }) => {
    requireAdmin(locals)

    // Load all Google sources (Drive and Gmail)
    const googleSources = await getGoogleSources()

    if (googleSources.length === 0) {
        throw error(404, 'No Google sources found. Please connect Google first.')
    }

    // Organize sources by type for easier access
    const driveSource = googleSources.find((s) => s.sourceType === 'google_drive')
    const gmailSource = googleSources.find((s) => s.sourceType === 'gmail')

    return {
        sources: googleSources,
        driveSource: driveSource || null,
        gmailSource: gmailSource || null,
    }
}

export const actions: Actions = {
    default: async ({ request, locals }) => {
        // Check if user is admin
        const session = locals.session
        const user = locals.user
        if (!user || user.role !== 'admin') {
            throw error(403, 'Admin access required')
        }

        const formData = await request.formData()

        // Get service configurations
        const driveEnabled = formData.has('driveEnabled')
        const gmailEnabled = formData.has('gmailEnabled')

        // Get user filter settings for Drive
        const driveUserFilterMode = (formData.get('driveUserFilterMode') as UserFilterMode) || 'all'
        const driveUserWhitelist =
            driveUserFilterMode === 'whitelist'
                ? (formData.getAll('driveUserWhitelist') as string[])
                : null
        const driveUserBlacklist =
            driveUserFilterMode === 'blacklist'
                ? (formData.getAll('driveUserBlacklist') as string[])
                : null

        // Get user filter settings for Gmail
        const gmailUserFilterMode = (formData.get('gmailUserFilterMode') as UserFilterMode) || 'all'
        const gmailUserWhitelist =
            gmailUserFilterMode === 'whitelist'
                ? (formData.getAll('gmailUserWhitelist') as string[])
                : null
        const gmailUserBlacklist =
            gmailUserFilterMode === 'blacklist'
                ? (formData.getAll('gmailUserBlacklist') as string[])
                : null

        // Validate whitelist requirements
        if (
            driveEnabled &&
            driveUserFilterMode === 'whitelist' &&
            (!driveUserWhitelist || driveUserWhitelist.length === 0)
        ) {
            throw error(400, 'Google Drive whitelist mode requires at least one user')
        }
        if (
            gmailEnabled &&
            gmailUserFilterMode === 'whitelist' &&
            (!gmailUserWhitelist || gmailUserWhitelist.length === 0)
        ) {
            throw error(400, 'Gmail whitelist mode requires at least one user')
        }

        try {
            // Update Google sources using the database operations module
            await updateGoogleSources(
                driveEnabled,
                gmailEnabled,
                {
                    userFilterMode: driveUserFilterMode,
                    userWhitelist: driveUserWhitelist,
                    userBlacklist: driveUserBlacklist,
                },
                {
                    userFilterMode: gmailUserFilterMode,
                    userWhitelist: gmailUserWhitelist,
                    userBlacklist: gmailUserBlacklist,
                },
            )

            // Trigger sync for enabled sources
            const googleConnectorUrl = process.env.GOOGLE_CONNECTOR_URL || 'http://localhost:3003'

            // Load active sources to trigger sync
            const activeSources = await getActiveGoogleSources()

            // Trigger sync for each active source
            for (const source of activeSources) {
                try {
                    await fetch(`${googleConnectorUrl}/sync/${source.id}`, {
                        method: 'POST',
                        headers: {
                            'Content-Type': 'application/json',
                        },
                    })
                } catch (err) {
                    // Log but don't fail if sync trigger fails
                    console.error(`Failed to trigger sync for source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Google integration settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        // Redirect back to integrations page with success message
        throw redirect(303, '/admin/integrations?success=google_configured')
    },
}
