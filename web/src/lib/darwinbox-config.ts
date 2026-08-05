import type { DarwinboxSourceConfig } from '$lib/types'

export interface DarwinboxConfigSelection {
    baseUrl: string
    readOnly: boolean
    participantMode: 'all' | 'allowlist'
    participantEmails: string[]
}

export function buildDarwinboxConfig(
    selection: DarwinboxConfigSelection,
    existing?: DarwinboxSourceConfig,
): DarwinboxSourceConfig {
    const baseUrl = validateBaseUrl(selection.baseUrl)
    const participantEmails = [
        ...new Set(selection.participantEmails.map((email) => email.trim().toLowerCase())),
    ]
    if (participantEmails.some((email) => !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email))) {
        throw new Error('Participant emails must be valid email addresses')
    }
    if (selection.participantMode === 'allowlist' && participantEmails.length === 0) {
        throw new Error(
            'At least one approved participant email is required when restricting actions to specific people',
        )
    }

    return {
        ...existing,
        base_url: baseUrl,
        read_only: selection.readOnly,
        authorization: {
            ...existing?.authorization,
            participant_mode: selection.participantMode,
            participant_emails: selection.participantMode === 'allowlist' ? participantEmails : [],
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
