import { AuthType } from '$lib/types'
import type { RemoteMcpConfig } from './client'

export interface RemoteMcpPutTransition {
    shouldBeActive: boolean
    shouldDeleteCredentials: boolean
    oauthBootstrapRequired: boolean
}

export function remoteMcpPutTransition(
    existingIsActive: boolean,
    previousConfig: Partial<RemoteMcpConfig>,
    nextConfig: RemoteMcpConfig,
): RemoteMcpPutTransition {
    const previousAuthType = previousConfig.auth_type ?? null
    const nextAuthType = nextConfig.auth_type ?? null
    const authTypeChanged = previousAuthType !== nextAuthType
    const endpointChanged = previousConfig.endpoint_url !== nextConfig.endpoint_url
    const oauthBootstrapRequired =
        nextAuthType === AuthType.OAUTH && (authTypeChanged || endpointChanged)

    return {
        shouldBeActive:
            nextAuthType === AuthType.OAUTH ? existingIsActive && !oauthBootstrapRequired : true,
        shouldDeleteCredentials:
            authTypeChanged ||
            (previousAuthType === AuthType.OAUTH &&
                nextAuthType === AuthType.OAUTH &&
                endpointChanged),
        oauthBootstrapRequired,
    }
}
