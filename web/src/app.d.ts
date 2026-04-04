// for information about these interfaces
declare global {
    namespace App {
        interface Locals {
            user: import('$lib/server/auth').SessionValidationResult['user']
            session: import('$lib/server/auth').SessionValidationResult['session']
            apiKeyAllowedSources: string[] | null
            apiKeyScope: 'public' | 'user' | 'admin' | null
            requestId: string
            logger: import('$lib/server/logger').Logger
        }
    }
}

export {}
