import { SourceType } from '$lib/types'
import { GoogleConnectorOAuthService } from './googleConnector'
import type { OAuthTokens } from './types'

/// Adapter that knows how to run the per-user OAuth flow for one provider.
/// New connectors plug in by adding an entry to `ADAPTERS_BY_SOURCE_TYPE`
/// below — the start/callback routes don't need to change.
export interface UserAuthAdapter {
    /// Provider label written to `service_credentials.provider` after a
    /// successful exchange.
    readonly provider: string

    /// True iff the connector's OAuth client is configured (entry exists in
    /// `connector_configs` for this provider). Surfaced as 412 to the caller
    /// when false.
    isConfigured(): Promise<boolean>

    /// Authorization URL the user is redirected to. `requiredScopes` is the
    /// set of write scopes the callback validates against the granted set.
    generateAuthUrl(args: {
        sourceId: string
        sourceType: string
        userId: string
        returnTo?: string
    }): Promise<{ url: string; requiredScopes: string[] }>

    /// Exchange the authorization code for tokens and resolve the user's
    /// principal email. Caller validates state, scopes, and writes the
    /// `service_credentials` row.
    exchangeCode(args: {
        sourceId: string
        code: string
        stateToken: string
    }): Promise<{ tokens: OAuthTokens; state: UserAuthState; principalEmail: string }>
}

/// Shape of the OAuth state we created in `generateAuthUrl` and consume in
/// `exchangeCode`. Only the fields the callback route reads are typed here;
/// the adapter is free to add provider-specific entries to `metadata`.
export interface UserAuthState {
    user_id: string
    metadata?: {
        requiredScopes?: string[]
        returnTo?: string
        [key: string]: unknown
    }
}

const googleAdapter: UserAuthAdapter = {
    provider: 'google',
    isConfigured: () => GoogleConnectorOAuthService.isConfigured(),
    generateAuthUrl: ({ sourceId, sourceType, userId, returnTo }) =>
        GoogleConnectorOAuthService.generateUserWriteAuthUrl(
            sourceId,
            sourceType,
            userId,
            returnTo,
        ),
    exchangeCode: async ({ sourceId, code, stateToken }) => {
        const { tokens, state } = await GoogleConnectorOAuthService.exchangeUserWriteCode(
            sourceId,
            code,
            stateToken,
        )
        const principalEmail = await GoogleConnectorOAuthService.fetchUserEmail(tokens.access_token)
        return { tokens, state: state as UserAuthState, principalEmail }
    },
}

const ADAPTERS_BY_SOURCE_TYPE: Partial<Record<SourceType, UserAuthAdapter>> = {
    [SourceType.GOOGLE_DRIVE]: googleAdapter,
    [SourceType.GMAIL]: googleAdapter,
}

export function getUserAuthAdapter(sourceType: string): UserAuthAdapter | undefined {
    return ADAPTERS_BY_SOURCE_TYPE[sourceType as SourceType]
}
