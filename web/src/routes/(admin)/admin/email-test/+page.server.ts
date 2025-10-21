import { fail } from '@sveltejs/kit'
import { EmailService } from '$lib/server/email'
import { requireAdmin } from '$lib/server/authHelpers'
import { z } from 'zod'
import type { Actions, PageServerLoad } from './$types'

const testEmailSchema = z.object({
    email: z.string().email('Please enter a valid email address'),
})

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    // Test email connection on page load
    const connectionTest = await EmailService.testConnection()

    return {
        connectionStatus: connectionTest,
    }
}

export const actions: Actions = {
    test: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const email = formData.get('email')?.toString()

        const validation = testEmailSchema.safeParse({ email })
        if (!validation.success) {
            return fail(400, {
                error: validation.error.issues[0].message,
                email,
            })
        }

        const { email: validEmail } = validation.data

        // Send a test magic link
        const testUrl = 'https://example.com/test-magic-link'
        const result = await EmailService.sendMagicLink(validEmail, testUrl, false)

        if (!result.success) {
            return fail(500, {
                error: result.error || 'Failed to send test email',
                email: validEmail,
            })
        }

        return {
            success: true,
            message: `Test email sent successfully to ${validEmail}`,
            messageId: result.messageId,
        }
    },
}
