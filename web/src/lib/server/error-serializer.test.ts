import { describe, expect, it } from 'vitest'
import { serializeError } from './error-serializer'

describe('serializeError', () => {
    it('includes PostgreSQL failure metadata without messages or query values', () => {
        const cause = Object.assign(new Error('database message with super-secret-value'), {
            name: 'PostgresError',
            code: '23514',
            severity: 'ERROR',
            schema_name: 'public',
            table_name: 'user_oauth_credentials',
            column_name: undefined,
            type_name: undefined,
            constraint_name: 'user_oauth_credentials_refresh_token_encrypted',
            routine: 'ExecConstraints',
            detail: 'Failing row contains super-secret-profile-data',
            query: 'INSERT with super-secret-driver-query',
            parameters: ['super-secret-token'],
        })
        const error = Object.assign(
            new Error('Failed query: INSERT with super-secret-query\nparams: super-secret-token'),
            {
                query: 'INSERT with super-secret-query',
                params: ['super-secret-token'],
                cause,
            },
        )

        const serialized = serializeError(error)

        expect(serialized).toMatchObject({
            type: 'Error',
            message: 'Database query failed',
            database: {
                sqlState: '23514',
                severity: 'ERROR',
                schema: 'public',
                table: 'user_oauth_credentials',
                constraint: 'user_oauth_credentials_refresh_token_encrypted',
                routine: 'ExecConstraints',
            },
        })

        const logged = JSON.stringify(serialized)
        expect(logged).not.toContain('super-secret')
        expect(logged).not.toContain('params')
        expect(logged).not.toContain('detail')
    })

    it('sanitizes query errors without a PostgreSQL cause', () => {
        const error = Object.assign(new Error('Failed query: SELECT $1\nparams: secret'), {
            query: 'SELECT $1',
            params: ['secret'],
            cause: Object.assign(new Error('connection failed'), { code: 'ECONNRESET' }),
        })

        const serialized = serializeError(error)

        expect(serialized.message).toBe('Database query failed')
        expect(serialized.database).toBeUndefined()
        expect(JSON.stringify(serialized)).not.toContain('secret')
    })

    it('extracts metadata from direct PostgreSQL errors', () => {
        const error = Object.assign(new Error('duplicate value: super-secret-email'), {
            name: 'PostgresError',
            code: '23505',
            severity: 'ERROR',
            table_name: 'users',
            constraint_name: 'users_email_key',
        })

        const serialized = serializeError(error)

        expect(serialized).toMatchObject({
            type: 'PostgresError',
            message: 'Database query failed',
            database: {
                sqlState: '23505',
                severity: 'ERROR',
                table: 'users',
                constraint: 'users_email_key',
            },
        })
        expect(JSON.stringify(serialized)).not.toContain('super-secret-email')
    })

    it('preserves ordinary error messages and stacks', () => {
        const error = new Error('ordinary failure')

        expect(serializeError(error)).toEqual({
            type: 'Error',
            message: 'ordinary failure',
            stack: error.stack,
            database: undefined,
        })
    })
})
