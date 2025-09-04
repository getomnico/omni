import { error, redirect } from '@sveltejs/kit'
import { MagicLinkService } from '$lib/server/magicLinks'
import { DomainService } from '$lib/server/domains'
import { createUserSession } from '$lib/server/auth'
import { db } from '$lib/server/db'
import { user, magicLinks } from '$lib/server/db/schema'
import { createId } from '@paralleldrive/cuid2'
import { eq } from 'drizzle-orm'
import { createHash } from 'crypto'
import type { PageServerLoad } from './$types'

export const load: PageServerLoad = async ({ url, cookies }) => {
    const token = url.searchParams.get('token')

    if (!token) {
        throw error(400, 'Missing magic link token')
    }

    const verificationResult = await MagicLinkService.verifyMagicLink(token)

    if (!verificationResult.success) {
        throw error(400, verificationResult.error || 'Invalid magic link')
    }

    // If user exists, create session and redirect
    if (verificationResult.user) {
        const sessionResult = await createUserSession(verificationResult.user.id)
        if (sessionResult.success && sessionResult.session) {
            cookies.set('session', sessionResult.session.token, {
                httpOnly: true,
                secure: true,
                sameSite: 'lax',
                path: '/',
                maxAge: 60 * 60 * 24 * 30, // 30 days
            })

            throw redirect(302, '/dashboard')
        }
    }

    // If user needs to be registered, check if their domain is approved
    if (verificationResult.requiresRegistration) {
        // Extract email from the magic link (we need to get it from the token)
        const magicLinkRecord = await db
            .select({ email: magicLinks.email })
            .from(magicLinks)
            .where(eq(magicLinks.tokenHash, createHash('sha256').update(token).digest('hex')))
            .limit(1)

        if (magicLinkRecord.length === 0) {
            throw error(400, 'Invalid magic link')
        }

        const email = magicLinkRecord[0].email
        const domain = DomainService.extractDomainFromEmail(email)

        if (!domain) {
            throw error(400, 'Invalid email domain')
        }

        const isDomainApproved = await DomainService.isDomainApproved(domain)

        if (!isDomainApproved) {
            throw error(403, 'Your email domain is not approved for this Omni instance')
        }

        // Auto-register the user
        const userId = createId()
        const newUser = {
            id: userId,
            email,
            role: 'user' as const,
            authMethod: 'magic_link' as const,
            domain,
            isActive: true,
            createdAt: new Date(),
            updatedAt: new Date(),
        }

        await db.insert(user).values(newUser)

        // Create session for the new user
        const sessionResult = await createUserSession(userId)
        if (sessionResult.success && sessionResult.session) {
            cookies.set('session', sessionResult.session.token, {
                httpOnly: true,
                secure: true,
                sameSite: 'lax',
                path: '/',
                maxAge: 60 * 60 * 24 * 30, // 30 days
            })

            throw redirect(302, '/dashboard')
        }
    }

    throw error(500, 'Failed to process magic link')
}
