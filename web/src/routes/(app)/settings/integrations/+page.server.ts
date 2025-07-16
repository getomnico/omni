import { requireAuth } from '$lib/server/authHelpers'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { UserOAuthCredentialsService } from '$lib/server/oauth/userCredentials'
import type { PageServerLoad } from './$types'

export const load: PageServerLoad = async ({ locals, url }) => {
    await requireAuth(locals.user)

    // Get all organization-level connected sources (read-only for users)
    const connectedSources = await db.select().from(sources)

    // Get user's OAuth credentials
    const userOAuthCredentials = await UserOAuthCredentialsService.getUserOAuthCredentials(
        locals.user.id,
    )

    // Handle success/error messages from OAuth operations
    const success = url.searchParams.get('success')
    const error = url.searchParams.get('error')

    let message = null
    if (success) {
        switch (success) {
            case 'google_unlinked':
                message = {
                    type: 'success',
                    text: 'Google account has been successfully unlinked.',
                }
                break
            case 'google_linked':
                message = { type: 'success', text: 'Google account has been successfully linked.' }
                break
        }
    }

    if (error) {
        switch (error) {
            case 'oauth_not_configured':
                message = {
                    type: 'error',
                    text: 'Google Sign-in is not configured. Please contact your administrator.',
                }
                break
            case 'oauth_unlink_error':
                message = {
                    type: 'error',
                    text: 'An error occurred while unlinking your Google account.',
                }
                break
            case 'rate_limit':
                message = { type: 'error', text: 'Too many requests. Please try again later.' }
                break
            default:
                message = { type: 'error', text: 'An unexpected error occurred.' }
        }
    }

    return {
        connectedSources,
        userOAuthCredentials,
        message,
        availableIntegrations: [
            {
                id: 'google',
                name: 'Google Workspace',
                description: 'Google Drive, Docs, Gmail, and more',
                icon: '🔗',
                connected: connectedSources.some(
                    (source) =>
                        source.sourceType === 'google_drive' || source.sourceType === 'gmail',
                ),
            },
            {
                id: 'slack',
                name: 'Slack',
                description: 'Slack messages and files',
                icon: '💬',
                connected: connectedSources.some((source) => source.sourceType === 'slack'),
            },
            {
                id: 'atlassian',
                name: 'Atlassian',
                description: 'Confluence and Jira',
                icon: '📚',
                connected: connectedSources.some(
                    (source) => source.sourceType === 'confluence' || source.sourceType === 'jira',
                ),
            },
        ],
    }
}
