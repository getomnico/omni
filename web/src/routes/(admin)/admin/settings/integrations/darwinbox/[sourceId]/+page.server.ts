import { error, fail, redirect } from '@sveltejs/kit'
import type { Actions, PageServerLoad } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { SourceType, type DarwinboxSourceConfig } from '$lib/types'
import { buildDarwinboxConfig, extractApiError } from '$lib/darwinbox-config'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)
    const source = await getSourceById(params.sourceId)
    if (!source) throw error(404, 'Source not found')
    if (source.sourceType !== SourceType.DARWINBOX) throw error(400, 'Invalid source type')
    return { source }
}

function list(form: FormData, name: string): string[] {
    return form
        .getAll(name)
        .map(String)
        .flatMap((value) => value.split(','))
        .map((value) => value.trim())
        .filter(Boolean)
}

export const actions: Actions = {
    default: async ({ request, params, locals, fetch }) => {
        requireAdmin(locals)
        const source = await getSourceById(params.sourceId)
        if (!source) throw error(404, 'Source not found')
        if (source.sourceType !== SourceType.DARWINBOX) throw error(400, 'Invalid source type')
        const form = await request.formData()
        const previous = source.config as DarwinboxSourceConfig
        try {
            const candidate = buildDarwinboxConfig(
                {
                    baseUrl: previous.base_url,
                    readOnly: form.has('read_only'),
                    participantMode:
                        form.get('participant_mode') === 'allowlist' ? 'allowlist' : 'all',
                    participantEmails: list(form, 'participant_emails').map((email) =>
                        email.toLowerCase(),
                    ),
                },
                previous,
            )

            const needsSync =
                JSON.stringify(previous.authorization?.participant_emails) !==
                JSON.stringify(candidate.authorization?.participant_emails)
            await updateSourceById(source.id, { config: candidate })
            if (needsSync && source.isActive) {
                const response = await fetch(`/api/sources/${source.id}/sync`, {
                    method: 'POST',
                    headers: { 'content-type': 'application/json' },
                    body: JSON.stringify({ sync_mode: 'full' }),
                })
                if (response.status === 409)
                    return fail(409, {
                        message:
                            'Configuration saved, but reconciliation did not start because another sync is active. Retry a full sync after it finishes.',
                    })
                if (!response.ok)
                    return fail(response.status, {
                        message: await extractApiError(
                            response,
                            'Configuration saved, but full reconciliation failed to start',
                        ),
                    })
            }
        } catch (cause) {
            return fail(400, {
                message:
                    cause instanceof Error
                        ? cause.message
                        : 'Unable to save Darwinbox configuration',
            })
        }
        throw redirect(303, '/admin/settings/integrations')
    },
}
