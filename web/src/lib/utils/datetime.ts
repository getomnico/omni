const DEFAULT_TIMEZONE = 'UTC'

export type DateInput = Date | string | number | null | undefined

function toDate(value: DateInput): Date | null {
    if (value === null || value === undefined) return null
    const date = value instanceof Date ? value : new Date(value)
    return Number.isNaN(date.getTime()) ? null : date
}

function safeTimeZone(timeZone: string | null | undefined): string {
    if (!timeZone) return DEFAULT_TIMEZONE
    try {
        new Intl.DateTimeFormat('en-US', { timeZone }).format(new Date())
        return timeZone
    } catch {
        return DEFAULT_TIMEZONE
    }
}

export function formatDate(value: DateInput, timeZone?: string | null): string {
    const date = toDate(value)
    if (!date) return '-'
    return new Intl.DateTimeFormat('en-US', {
        year: 'numeric',
        month: 'short',
        day: 'numeric',
        timeZone: safeTimeZone(timeZone),
    }).format(date)
}

export function formatDateTime(value: DateInput, timeZone?: string | null): string {
    const date = toDate(value)
    if (!date) return '-'
    return new Intl.DateTimeFormat('en-US', {
        year: 'numeric',
        month: 'short',
        day: 'numeric',
        hour: 'numeric',
        minute: '2-digit',
        timeZone: safeTimeZone(timeZone),
    }).format(date)
}

export function formatTime(value: DateInput, timeZone?: string | null): string {
    const date = toDate(value)
    if (!date) return '-'
    return new Intl.DateTimeFormat('en-US', {
        hour: 'numeric',
        minute: '2-digit',
        hour12: true,
        timeZone: safeTimeZone(timeZone),
    })
        .format(date)
        .toLowerCase()
}

function calendarDayKey(value: Date, timeZone: string): string {
    const parts = new Intl.DateTimeFormat('en-US', {
        year: 'numeric',
        month: '2-digit',
        day: '2-digit',
        timeZone,
    }).formatToParts(value)
    const get = (type: string) => parts.find((part) => part.type === type)?.value ?? ''
    return `${get('year')}-${get('month')}-${get('day')}`
}

export function formatChatTimestamp(value: DateInput, timeZone?: string | null): string {
    const date = toDate(value)
    if (!date) return ''

    const zone = safeTimeZone(timeZone)
    const todayKey = calendarDayKey(new Date(), zone)
    const dateKey = calendarDayKey(date, zone)
    if (dateKey === todayKey) return formatTime(date, zone)

    return new Intl.DateTimeFormat('en-US', {
        month: 'short',
        day: 'numeric',
        timeZone: zone,
    }).format(date)
}
