// for information about these interfaces
declare global {
    namespace App {
        interface Locals {
            user: import('$lib/server/auth').SessionValidationResult['user']
            session: import('$lib/server/auth').SessionValidationResult['session']
            apiKeyAllowedSources: string[] | null
            apiKeyScope: 'public' | 'user' | null
            db: import('drizzle-orm/postgres-js').PostgresJsDatabase<
                typeof import('$lib/server/db/schema')
            >
            requestId: string
            logger: import('$lib/server/logger').Logger
        }
    }
}

export {}
