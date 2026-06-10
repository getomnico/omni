export const MIN_SYNC_INTERVAL_SECONDS = 60
export const MAX_SYNC_INTERVAL_SECONDS = 365 * 24 * 60 * 60

export type SyncIntervalUnit = 'minutes' | 'hours' | 'days'

export const SYNC_INTERVAL_UNIT_SECONDS: Record<SyncIntervalUnit, number> = {
    minutes: 60,
    hours: 60 * 60,
    days: 24 * 60 * 60,
}

export const SYNC_INTERVAL_PRESETS = [
    { value: '1800', seconds: 30 * 60, label: 'Every 30 minutes' },
    { value: '3600', seconds: 60 * 60, label: 'Every hour' },
    { value: '21600', seconds: 6 * 60 * 60, label: 'Every 6 hours' },
    { value: '43200', seconds: 12 * 60 * 60, label: 'Every 12 hours' },
    { value: '86400', seconds: 24 * 60 * 60, label: 'Every day' },
    { value: '604800', seconds: 7 * 24 * 60 * 60, label: 'Every week' },
] as const

function pluralize(value: number, singular: string, plural = `${singular}s`) {
    return value === 1 ? singular : plural
}

export function isValidSyncIntervalSeconds(seconds: unknown): seconds is number {
    return (
        typeof seconds === 'number' &&
        Number.isInteger(seconds) &&
        seconds >= MIN_SYNC_INTERVAL_SECONDS &&
        seconds <= MAX_SYNC_INTERVAL_SECONDS
    )
}

export function formatSyncInterval(seconds: number | null | undefined): string {
    if (!seconds || seconds <= 0) {
        return 'Not configured'
    }

    const preset = SYNC_INTERVAL_PRESETS.find((option) => option.seconds === seconds)
    if (preset) {
        return preset.label
    }

    const weekSeconds = 7 * SYNC_INTERVAL_UNIT_SECONDS.days
    if (seconds % weekSeconds === 0) {
        const weeks = seconds / weekSeconds
        return `Every ${weeks} ${pluralize(weeks, 'week')}`
    }

    if (seconds % SYNC_INTERVAL_UNIT_SECONDS.days === 0) {
        const days = seconds / SYNC_INTERVAL_UNIT_SECONDS.days
        return `Every ${days} ${pluralize(days, 'day')}`
    }

    if (seconds % SYNC_INTERVAL_UNIT_SECONDS.hours === 0) {
        const hours = seconds / SYNC_INTERVAL_UNIT_SECONDS.hours
        return `Every ${hours} ${pluralize(hours, 'hour')}`
    }

    if (seconds % SYNC_INTERVAL_UNIT_SECONDS.minutes === 0) {
        const minutes = seconds / SYNC_INTERVAL_UNIT_SECONDS.minutes
        return `Every ${minutes} ${pluralize(minutes, 'minute')}`
    }

    return `Every ${seconds} ${pluralize(seconds, 'second')}`
}

export function deriveCustomSyncInterval(seconds: number | null | undefined): {
    value: number
    unit: SyncIntervalUnit
} {
    if (!seconds || seconds <= 0) {
        return { value: 1, unit: 'hours' }
    }

    if (seconds % SYNC_INTERVAL_UNIT_SECONDS.days === 0) {
        return { value: seconds / SYNC_INTERVAL_UNIT_SECONDS.days, unit: 'days' }
    }

    if (seconds % SYNC_INTERVAL_UNIT_SECONDS.hours === 0) {
        return { value: seconds / SYNC_INTERVAL_UNIT_SECONDS.hours, unit: 'hours' }
    }

    return {
        value: Math.max(1, Math.round(seconds / SYNC_INTERVAL_UNIT_SECONDS.minutes)),
        unit: 'minutes',
    }
}
