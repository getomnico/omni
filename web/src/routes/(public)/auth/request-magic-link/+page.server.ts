import { fail, redirect } from '@sveltejs/kit'
import { MagicLinkService } from '$lib/server/magicLinks'
import { DomainService } from '$lib/server/domains'
import { EmailService } from '$lib/server/email'
import { db } from '$lib/server/db'
import { user } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'
import { z } from 'zod'
import type { Actions } from './$types'

const magicLinkRequestSchema = z.object({
    email: z.string().email('Please enter a valid email address').min(1, 'Email is required'),
})

export const actions: Actions = {
    default: async ({ request }) => {
        const formData = await request.formData()
        const email = formData.get('email')?.toString()?.toLowerCase()

        const validation = magicLinkRequestSchema.safeParse({ email })
        if (!validation.success) {
            return fail(400, {
                error: validation.error.issues[0].message,
                email,
            })
        }

        const { email: validEmail } = validation.data

        try {
            // Check if user exists
            const existingUser = await db
                .select()
                .from(user)
                .where(eq(user.email, validEmail))
                .limit(1)

            let canAccessSystem = false
            let isNewUser = false

            if (existingUser.length > 0) {
                // User exists, check if they can use magic links
                const userRecord = existingUser[0]
                canAccessSystem =
                    userRecord.authMethod === 'magic_link' || userRecord.authMethod === 'both'
            } else {
                // User doesn't exist, check if their domain is approved
                const domain = DomainService.extractDomainFromEmail(validEmail)
                if (domain) {
                    const isDomainApproved = await DomainService.isDomainApproved(domain)
                    if (isDomainApproved) {
                        canAccessSystem = true
                        isNewUser = true
                    }
                }
            }

            if (!canAccessSystem) {
                return fail(403, {
                    error: 'Your email domain is not approved for this Omni instance, or magic link authentication is not enabled for your account.',
                    email: validEmail,
                })
            }

            // Create magic link
            const magicLinkResult = await MagicLinkService.createMagicLink(
                validEmail,
                existingUser.length > 0 ? existingUser[0].id : undefined,
            )

            if (!magicLinkResult.success) {
                return fail(500, {
                    error: 'Failed to create magic link. Please try again.',
                    email: validEmail,
                })
            }

            // Send email
            const magicLinkUrl = MagicLinkService.generateMagicLinkUrl(
                magicLinkResult.magicLink!.token,
                request.url.split('/auth/request-magic-link')[0],
            )

            const emailResult = await EmailService.sendMagicLink(
                validEmail,
                magicLinkUrl,
                isNewUser,
            )

            if (!emailResult.success) {
                return fail(500, {
                    error: 'Failed to send magic link email. Please try again.',
                    email: validEmail,
                })
            }

            // Success - redirect to confirmation page
            throw redirect(302, `/auth/magic-link-sent?email=${encodeURIComponent(validEmail)}`)
        } catch (error) {
            console.error('Error processing magic link request:', error)
            return fail(500, {
                error: 'An unexpected error occurred. Please try again.',
                email: validEmail,
            })
        }
    },
}
