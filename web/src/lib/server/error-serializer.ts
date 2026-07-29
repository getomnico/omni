export interface SerializedDatabaseError {
    sqlState: string
    severity?: string
    schema?: string
    table?: string
    column?: string
    dataType?: string
    constraint?: string
    routine?: string
}

export interface SerializedError {
    type: string
    message: string
    stack?: string
    database?: SerializedDatabaseError
}

type ErrorRecord = Record<string, unknown>

function asErrorRecord(value: unknown): ErrorRecord | null {
    return typeof value === 'object' && value !== null ? (value as ErrorRecord) : null
}

function getString(record: ErrorRecord, key: string): string | undefined {
    const value = record[key]
    return typeof value === 'string' ? value : undefined
}

function serializeDatabaseError(cause: unknown): SerializedDatabaseError | undefined {
    const record = asErrorRecord(cause)
    if (!record) return undefined

    const sqlState = getString(record, 'code')
    if (!sqlState || !/^[0-9A-Z]{5}$/.test(sqlState)) return undefined

    return {
        sqlState,
        severity: getString(record, 'severity'),
        schema: getString(record, 'schema_name'),
        table: getString(record, 'table_name'),
        column: getString(record, 'column_name'),
        dataType: getString(record, 'data_type_name') ?? getString(record, 'type_name'),
        constraint: getString(record, 'constraint_name'),
        routine: getString(record, 'routine'),
    }
}

function stackFrames(stack: string | undefined): string | undefined {
    if (!stack) return undefined

    const frames = stack.split('\n').filter((line) => line.trimStart().startsWith('at '))
    return frames.length > 0 ? frames.join('\n') : undefined
}

export function serializeError(error: Error): SerializedError {
    const record = error as Error & ErrorRecord
    const isQueryError = typeof record.query === 'string' && Array.isArray(record.params)
    const database = serializeDatabaseError(record.cause) ?? serializeDatabaseError(record)
    const isDatabaseError = isQueryError || database !== undefined

    return {
        type: error.name,
        message: isDatabaseError ? 'Database query failed' : error.message,
        stack: isDatabaseError ? stackFrames(error.stack) : error.stack,
        database,
    }
}
