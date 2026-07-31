import type {
    DarwinboxActionCapability,
    DarwinboxEmployeeField,
    DarwinboxEmployeeScope,
    DarwinboxManifestExtraSchema,
    DarwinboxSourceConfig,
} from '$lib/types'

export const DARWINBOX_EMPLOYEE_FIELDS: readonly DarwinboxEmployeeField[] = [
    'name',
    'employee_id',
    'company_email',
    'department',
    'designation',
    'office_location',
    'manager_employee_id',
    'employee_type',
    'cost_center',
    'work_country',
    'grade',
    'band',
    'confirmation_status',
    'employment_dates',
]

export interface DarwinboxConfigSelection {
    baseUrl: string
    readOnly: boolean
    selectedSyncModules: string[]
    selectedActions: string[]
    participantMode: 'all' | 'allowlist'
    participantEmails: string[]
    employeeScope: DarwinboxEmployeeScope | null
    employeeFields: DarwinboxEmployeeField[]
    writeAcknowledged: boolean
}

export function availableActions(
    schema?: DarwinboxManifestExtraSchema,
): DarwinboxActionCapability[] {
    return (schema?.action_capabilities ?? []).filter((action) => action.available !== false)
}

export function buildDarwinboxConfig(
    selection: DarwinboxConfigSelection,
    schema?: DarwinboxManifestExtraSchema,
    existing?: DarwinboxSourceConfig,
): DarwinboxSourceConfig {
    const baseUrl = validateBaseUrl(selection.baseUrl)
    const participantEmails = [
        ...new Set(selection.participantEmails.map((email) => email.trim().toLowerCase())),
    ]
    if (participantEmails.some((email) => !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email))) {
        throw new Error('Participant emails must be valid email addresses')
    }
    const syncNames = new Set(
        (schema?.sync_capabilities ?? []).filter((c) => c.available).map((c) => c.name),
    )
    const actions = availableActions(schema)
    const allowedNames = new Set(
        actions.filter((a) => !selection.readOnly || a.mode === 'read').map((a) => a.name),
    )
    const selectedActions = [
        ...new Set(selection.selectedActions.filter((name) => allowedNames.has(name))),
    ]
    const selectedModules = new Set(
        actions.filter((a) => selectedActions.includes(a.name) && a.module).map((a) => a.module),
    )
    const employeeFields = [
        ...new Set(selection.employeeFields.filter((f) => DARWINBOX_EMPLOYEE_FIELDS.includes(f))),
    ]
    const peopleEnabled =
        syncNames.has('employee_directory') &&
        selection.selectedSyncModules.includes('employee_directory')
    if (peopleEnabled && !validEmployeeScope(selection.employeeScope)) {
        throw new Error(
            "People directory scope must be 'all' or contain at least one employee ID, email, or department",
        )
    }
    if (peopleEnabled && !employeeFields.includes('company_email')) {
        throw new Error(
            'Select company email as the canonical organization-visible People identity',
        )
    }
    const hasSelectedWrite =
        !selection.readOnly &&
        selectedActions.some(
            (name) => actions.find((action) => action.name === name)?.mode === 'write',
        )
    if (hasSelectedWrite && !selection.writeAcknowledged) {
        throw new Error('Confirm write-mode acknowledgement before continuing')
    }
    if (
        selection.participantMode === 'allowlist' &&
        selectedActions.length > 0 &&
        participantEmails.length === 0
    ) {
        throw new Error(
            'At least one approved participant email is required when restricting actions to specific people',
        )
    }

    const syncModules = { ...(existing?.sync_modules ?? {}) }
    for (const key of Object.keys(syncModules)) syncModules[key] = false
    for (const name of selection.selectedSyncModules)
        if (syncNames.has(name)) syncModules[name] = true

    const actionModules = { ...(existing?.action_modules ?? {}) }
    for (const key of Object.keys(actionModules)) actionModules[key] = false
    for (const module of selectedModules) actionModules[module] = true

    return {
        ...existing,
        base_url: baseUrl,
        read_only: selection.readOnly,
        sync_modules: syncModules,
        action_modules: actionModules,
        employee_scope: peopleEnabled
            ? selection.employeeScope
            : (existing?.employee_scope ?? null),
        employee_fields: peopleEnabled ? employeeFields : (existing?.employee_fields ?? []),
        authorization: {
            ...existing?.authorization,
            actions_enabled: selectedActions.length > 0,
            write_acknowledged: hasSelectedWrite ? selection.writeAcknowledged : false,
            participant_mode: selection.participantMode,
            participant_emails:
                selection.participantMode === 'allowlist' && selectedActions.length > 0
                    ? participantEmails
                    : [],
            allowed_actions: selectedActions,
            allowed_report_ids: existing?.authorization?.allowed_report_ids ?? [],
            max_batch_size: existing?.authorization?.max_batch_size ?? 1,
        },
    }
}

function validateBaseUrl(value: string): string {
    let url: URL
    try {
        url = new URL(value.trim())
    } catch {
        throw new Error('Darwinbox base URL must be a valid URL')
    }
    if (url.username || url.password)
        throw new Error('Darwinbox base URL must not contain credentials')
    const loopback =
        url.hostname === 'localhost' || url.hostname === '127.0.0.1' || url.hostname === '::1'
    if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) {
        throw new Error('Darwinbox base URL must use HTTPS (HTTP is allowed only for loopback)')
    }
    return url.href.replace(/\/$/, '')
}

export function validEmployeeScope(scope: DarwinboxEmployeeScope | null): boolean {
    return (
        scope?.mode === 'all' ||
        (scope?.mode === 'include' &&
            Boolean(
                scope.employee_ids?.length ||
                scope.employee_emails?.length ||
                scope.departments?.length,
            ))
    )
}

export async function extractApiError(response: Response, fallback: string): Promise<string> {
    try {
        const body: unknown = await response.json()
        if (body && typeof body === 'object') {
            const value = body as { message?: unknown; validation?: unknown; errors?: unknown }
            const list = Array.isArray(value.validation)
                ? value.validation
                : Array.isArray(value.errors)
                  ? value.errors
                  : []
            const messages = list.filter((item): item is string => typeof item === 'string')
            if (typeof value.message === 'string' && value.message.trim())
                messages.unshift(value.message)
            if (messages.length) return [...new Set(messages)].join('\n')
        }
    } catch {
        /* non-JSON response */
    }
    return fallback
}
