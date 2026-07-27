import { error, redirect } from '@sveltejs/kit'
import type { Actions, PageServerLoad } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { SourceType, type DarwinboxSourceConfig } from '$lib/types'

/** All known Darwinbox read action names. */
const READ_ACTIONS: string[] = [
    'find_employee',
    'get_my_profile',
    'get_my_leave_balance',
    'get_my_leave_requests',
    'get_my_attendance',
    'get_my_timesheet',
    'get_holiday_calendar',
    'list_pending_leave_approvals',
    'get_team_leave_calendar',
    'get_team_attendance_exceptions',
    'get_direct_report_profile',
    'fetch_report_ids',
    'run_report',
]

/** All known Darwinbox write action names. */
const WRITE_ACTIONS: string[] = [
    'apply_my_leave',
    'revoke_my_leave',
    'regularize_my_attendance',
    'approve_leave_request',
    'reject_leave_request',
]

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)
    const source = await getSourceById(params.sourceId)
    if (!source) throw error(404, 'Source not found')
    if (source.sourceType !== SourceType.DARWINBOX) throw error(400, 'Invalid source type')
    return { source }
}

function csv(value: FormDataEntryValue | null): string[] {
    return String(value ?? '')
        .split(',')
        .map((item) => item.trim())
        .filter(Boolean)
}

export const actions: Actions = {
    default: async ({ request, params, locals, fetch }) => {
        requireAdmin(locals)
        const source = await getSourceById(params.sourceId)
        if (!source) throw error(404, 'Source not found')
        if (source.sourceType !== SourceType.DARWINBOX) throw error(400, 'Invalid source type')

        const form = await request.formData()
        const readOnly = form.has('read_only')
        const participants = csv(form.get('participant_emails'))
        const targetIds = csv(form.get('target_employee_ids'))
        const targetEmails = csv(form.get('target_employee_emails'))
        const targetDepartments = csv(form.get('target_departments'))
        const allowedActions = readOnly ? READ_ACTIONS : [...READ_ACTIONS, ...WRITE_ACTIONS]

        const previous = source.config as DarwinboxSourceConfig
        const candidate: DarwinboxSourceConfig = {
            ...previous,
            read_only: readOnly,
            employee_scope: {
                mode: 'include',
                employee_ids: targetIds,
            },
            employee_fields: [
                'name',
                'employee_id',
                'company_email',
                'department',
                'designation',
                'office_location',
            ],
            sync_modules: {
                employee_directory: true,
                deleted_employees: true,
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
                employee_self_service: true,
                manager_workflows: true,
                hr_operations: false,
                ats: false,
                reports: true,
            },
            authorization: {
                actions_enabled: true,
                write_acknowledged: form.has('write_acknowledged'),
                participant_emails: participants,
                target_employee_ids: targetIds,
                target_employee_emails: targetEmails,
                target_departments: targetDepartments,
                allowed_actions: allowedActions,
                hr_admin_emails: [],
                recruiter_emails: [],
                allowed_report_ids: [],
                max_batch_size: 1,
            },
        }

        const previousPolicy = { ...(source.config as DarwinboxSourceConfig), read_only: undefined }
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
