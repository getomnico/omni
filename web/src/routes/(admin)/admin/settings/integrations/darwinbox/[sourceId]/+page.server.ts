import { error, fail, redirect } from '@sveltejs/kit'
import type { Actions, PageServerLoad } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import {
    SourceType,
    type ConnectorListEntry,
    type DarwinboxEmployeeField,
    type DarwinboxManifestExtraSchema,
    type DarwinboxSourceConfig,
} from '$lib/types'
import { buildDarwinboxConfig, extractApiError } from '$lib/darwinbox-config'

async function manifest(fetcher: typeof fetch): Promise<DarwinboxManifestExtraSchema> {
    const response = await fetcher('/api/connectors')
    if (!response.ok)
        throw error(
            response.status,
            await extractApiError(response, 'Failed to load Darwinbox capabilities'),
        )
    const connectors: ConnectorListEntry[] = await response.json()
    const schema = connectors.find((connector) => connector.source_type === 'darwinbox')?.manifest
        .extra_schema
    if (!schema) throw error(503, 'Darwinbox connector is not registered')
    return schema
}

export const load: PageServerLoad = async ({ params, locals, fetch }) => {
    requireAdmin(locals)
    const source = await getSourceById(params.sourceId)
    if (!source) throw error(404, 'Source not found')
    if (source.sourceType !== SourceType.DARWINBOX) throw error(400, 'Invalid source type')
    return { source, manifest: await manifest(fetch) }
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
                    selectedSyncModules: list(form, 'sync_modules'),
                    selectedActions: list(form, 'allowed_actions'),
                    participantMode:
                        form.get('participant_mode') === 'allowlist' ? 'allowlist' : 'all',
                    participantEmails: list(form, 'participant_emails').map((email) =>
                        email.toLowerCase(),
                    ),
                    employeeScope:
                        form.get('scope_mode') === 'all'
                            ? { mode: 'all' }
                            : {
                                  mode: 'include',
                                  employee_ids: list(form, 'employee_ids'),
                                  employee_emails: list(form, 'employee_emails'),
                                  departments: list(form, 'departments'),
                              },
                    employeeFields: list(form, 'employee_fields') as DarwinboxEmployeeField[],
                    writeAcknowledged: form.has('write_acknowledged'),
                },
                await manifest(fetch),
                previous,
            )

            const peoplePolicy = (config: DarwinboxSourceConfig) =>
                JSON.stringify({
                    enabled: config.sync_modules?.employee_directory,
                    scope: config.employee_scope,
                    fields: config.employee_fields,
                    sync: config.sync_modules,
                })
            const needsSync = peoplePolicy(previous) !== peoplePolicy(candidate)
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
                            'Configuration saved, but People reconciliation did not start because another sync is active. Retry a full sync after it finishes.',
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
