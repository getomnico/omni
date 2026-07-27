import { error, redirect } from '@sveltejs/kit'
import type { Actions, PageServerLoad } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { SourceType, type DarwinboxSourceConfig } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals, fetch: serverFetch }) => {
    requireAdmin(locals)
    const source = await getSourceById(params.sourceId)
    if (!source) throw error(404, 'Source not found')
    if (source.sourceType !== SourceType.DARWINBOX) throw error(400, 'Invalid source type')
    const config = source.config as DarwinboxSourceConfig
    return { source }
}

function csv(value: FormDataEntryValue | null): string[] {
    return String(value ?? '')
        .split(',')
        .map((item) => item.trim())
        .filter(Boolean)
}

function positiveInteger(value: FormDataEntryValue | null, fallback: number): number {
    const parsed = Number(value ?? fallback)
    return Number.isInteger(parsed) ? parsed : Number.NaN
}

export const actions: Actions = {
    default: async ({ request, params, locals, fetch }) => {
        requireAdmin(locals)
        const source = await getSourceById(params.sourceId)
        if (!source) throw error(404, 'Source not found')
        if (source.sourceType !== SourceType.DARWINBOX) throw error(400, 'Invalid source type')

        const form = await request.formData()
        const previous = source.config as DarwinboxSourceConfig
        const readOnly = form.has('read_only')
        const participants = csv(form.get('participant_emails'))
        const allowedActions = csv(form.get('allowed_actions'))
        const employeeIds = csv(form.get('employee_ids'))
        const employeeEmails = csv(form.get('employee_emails'))
        const departments = csv(form.get('departments'))
        const targetIds = csv(form.get('target_employee_ids'))
        const targetEmails = csv(form.get('target_employee_emails'))
        const targetDepartments = csv(form.get('target_departments'))
        const employeeFields = csv(
            form.get('employee_fields'),
        ) as DarwinboxSourceConfig['employee_fields']
        const employeeDirectory = form.has('employee_directory')
        const selfService = form.has('employee_self_service')
        const managerWorkflows = form.has('manager_workflows')

        const candidate: DarwinboxSourceConfig = {
            ...previous,
            read_only: readOnly,
            employee_scope: employeeDirectory
                ? {
                      mode: 'include',
                      employee_ids: employeeIds,
                      employee_emails: employeeEmails,
                      departments,
                  }
                : null,
            employee_fields: employeeDirectory ? employeeFields : [],
            sync_modules: {
                employee_directory: employeeDirectory,
                deleted_employees: employeeDirectory,
                departments: false,
                designations: false,
                office_locations: false,
                business_units: false,
                divisions: false,
                cost_centers: false,
                group_companies: false,
                positions: false,
                holidays: false,
                ats_jobs: false,
            },
            action_modules: {
                employee_self_service: selfService,
                manager_workflows: managerWorkflows,
                hr_operations: false,
                ats: false,
                reports: false,
            },
            authorization: {
                ...previous.authorization,
                actions_enabled: selfService || managerWorkflows,
                write_acknowledged: form.has('write_acknowledged'),
                participant_emails: participants,
                target_employee_ids: targetIds,
                target_employee_emails: targetEmails,
                target_departments: targetDepartments,
                allowed_actions: allowedActions,
                hr_admin_emails: [],
                recruiter_emails: [],
                allowed_report_ids: [],
                max_batch_size: positiveInteger(form.get('max_batch_size'), 1),
                max_requests_per_minute: positiveInteger(form.get('max_requests_per_minute'), 10),
            },
        }

        const previousPolicy = { ...previous, read_only: undefined }
        const nextPolicy = { ...candidate, read_only: undefined }
        const policyChanged = JSON.stringify(previousPolicy) !== JSON.stringify(nextPolicy)
        await updateSourceById(source.id, { config: candidate })

        if (policyChanged && source.isActive) {
            const response = await fetch(`/api/sources/${source.id}/sync`, {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ sync_mode: 'full' }),
            })
            if (!response.ok && response.status !== 409) {
                throw error(
                    response.status,
                    'Configuration saved, but full reconciliation failed to start',
                )
            }
        }
        throw redirect(303, '/admin/settings/integrations')
    },
}
