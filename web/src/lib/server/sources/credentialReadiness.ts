import { getSourceById } from '$lib/server/db/sources'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'

export type CredentialReadiness =
    | { ready: true }
    | {
          ready: false
          reason:
              | 'needs_user_auth'
              | 'no_org_credentials'
              | 'source_not_found'
              | 'unsupported_source_type'
          oauth_start_url?: string
      }

/// Decides whether `userId` can invoke a write tool against `sourceId` using
/// existing credentials, without performing the call. Used by:
///   * the chat tool-approval card (renders Connect-CTA when not ready)
///   * the (app)/settings/integrations page (Connected / Connect status)
///
/// Mirrors the Rust-side `resolve_credentials` rule
/// (services/connector-manager/src/handlers.rs).
export async function getCredentialReadiness(
    sourceId: string,
    userId: string,
): Promise<CredentialReadiness> {
    const source = await getSourceById(sourceId)
    if (!source || source.isDeleted) {
        return { ready: false, reason: 'source_not_found' }
    }

    const perUserCred = await serviceCredentialsRepository.getByUserAndSource(sourceId, userId)
    if (perUserCred) {
        return { ready: true }
    }

    // Disambiguate: if there isn't even an org row to derive a provider from,
    // the source itself is misconfigured rather than just missing user auth.
    const orgRow = await serviceCredentialsRepository.getOrgCredsBySourceId(sourceId)
    if (!orgRow) {
        return { ready: false, reason: 'no_org_credentials' }
    }

    return {
        ready: false,
        reason: 'needs_user_auth',
        oauth_start_url: `/api/oauth/start?source_id=${sourceId}`,
    }
}
