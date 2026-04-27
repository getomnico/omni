import { and, eq, isNull } from 'drizzle-orm'
import { db } from '$lib/server/db'
import { sources, serviceCredentials } from '$lib/server/db/schema'

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
/// Mirrors the Rust-side resolution rule in
/// `ServiceCredentialsRepo::get_for_action`. Read-mode actions don't need this
/// check — they fall back to org credentials and won't 412.
export async function getCredentialReadiness(
    sourceId: string,
    userId: string,
): Promise<CredentialReadiness> {
    const [source] = await db
        .select({
            id: sources.id,
            sourceType: sources.sourceType,
            scope: sources.scope,
            isDeleted: sources.isDeleted,
        })
        .from(sources)
        .where(eq(sources.id, sourceId))
        .limit(1)

    if (!source || source.isDeleted) {
        return { ready: false, reason: 'source_not_found' }
    }

    // Personal sources: a single org-row credential covers everything for the
    // owning user. If it's missing the source is just unconfigured.
    if (source.scope === 'user') {
        const [orgCred] = await db
            .select({ id: serviceCredentials.id })
            .from(serviceCredentials)
            .where(
                and(eq(serviceCredentials.sourceId, sourceId), isNull(serviceCredentials.userId)),
            )
            .limit(1)
        return orgCred ? { ready: true } : { ready: false, reason: 'no_org_credentials' }
    }

    // Org-wide source: user must have a per-user credential row to run write
    // tools.
    const [perUserCred] = await db
        .select({ id: serviceCredentials.id })
        .from(serviceCredentials)
        .where(
            and(eq(serviceCredentials.sourceId, sourceId), eq(serviceCredentials.userId, userId)),
        )
        .limit(1)

    if (perUserCred) {
        return { ready: true }
    }

    // Confirm an org-row credential exists at all; if not the source itself is
    // misconfigured rather than just missing per-user auth.
    const [orgRow] = await db
        .select({ id: serviceCredentials.id })
        .from(serviceCredentials)
        .where(and(eq(serviceCredentials.sourceId, sourceId), isNull(serviceCredentials.userId)))
        .limit(1)
    if (!orgRow) {
        return { ready: false, reason: 'no_org_credentials' }
    }

    return {
        ready: false,
        reason: 'needs_user_auth',
        oauth_start_url: `/api/sources/${sourceId}/user-auth/start`,
    }
}
