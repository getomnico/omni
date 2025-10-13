import { fail, redirect } from '@sveltejs/kit'
import { hash } from '@node-rs/argon2'
import { ulid } from 'ulid'
import { generateSessionToken, createSession, setSessionTokenCookie } from '$lib/server/auth.js'
import { DomainService } from '$lib/server/domains.js'
import { userRepository } from '$lib/server/db/users'
import { SystemFlags } from '$lib/server/system-flags'
import type { Actions, PageServerLoad } from './$types.js'

export const load: PageServerLoad = async ({ locals }) => {
    if (locals.user) {
        throw redirect(302, '/')
    }

    // Check if this is first-time setup
    const isInitialized = await SystemFlags.isInitialized()
    const isFirstUser = !isInitialized

    return {
        isFirstUser,
    }
}

export const actions: Actions = {
    default: async ({ request, cookies }) => {
        const formData = await request.formData()
        const email = formData.get('email') as string
        const password = formData.get('password') as string
        const confirmPassword = formData.get('confirmPassword') as string

        // Basic validation
        if (!email || !password || !confirmPassword) {
            return fail(400, {
                error: 'All fields are required.',
                email,
            })
        }

        if (
            typeof email !== 'string' ||
            typeof password !== 'string' ||
            typeof confirmPassword !== 'string'
        ) {
            return fail(400, {
                error: 'Invalid form data.',
                email,
            })
        }

        // Email validation
        const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/
        if (!emailRegex.test(email)) {
            return fail(400, {
                error: 'Please enter a valid email address.',
                email,
            })
        }

        // Password validation
        if (password.length < 8) {
            return fail(400, {
                error: 'Password must be at least 8 characters long.',
                email,
            })
        }

        if (password !== confirmPassword) {
            return fail(400, {
                error: 'Passwords do not match.',
                email,
            })
        }

        // Check if this is the first user
        const isInitialized = await SystemFlags.isInitialized()
        const isFirstUser = !isInitialized

        try {
            // Check if email already exists
            const emailExists = await userRepository.emailExists(email)
            if (emailExists) {
                return fail(400, {
                    error: 'An account with this email already exists.',
                    email,
                })
            }

            // Hash password
            const passwordHash = await hash(password, {
                memoryCost: 65536,
                timeCost: 3,
                outputLen: 32,
                parallelism: 1,
            })

            // Extract domain from email
            const domain = DomainService.extractDomainFromEmail(email.toLowerCase())

            // Create user
            const newUserId = ulid()
            await userRepository.create({
                id: newUserId,
                email: email.toLowerCase(),
                passwordHash,
                role: isFirstUser ? 'admin' : 'user',
                authMethod: 'password',
                domain,
                isActive: true,
                createdAt: new Date(),
            })

            // If this is the first user (admin), auto-approve their domain and mark system as initialized
            if (isFirstUser && domain) {
                await DomainService.autoApproveDomainForAdmin(newUserId)
                await SystemFlags.markAsInitialized()
            }

            // Create session and log the user in
            const sessionToken = generateSessionToken()
            const session = await createSession(sessionToken, newUserId)
            setSessionTokenCookie(cookies, sessionToken, session.expiresAt)
        } catch (error) {
            console.error('Registration error:', error)
            return fail(500, {
                error: 'An unexpected error occurred. Please try again.',
                email,
            })
        }

        // Redirect based on whether this was the first user
        if (isFirstUser) {
            // Redirect first admin to integrations page to set up data sources
            throw redirect(302, '/admin/integrations')
        } else {
            // Redirect regular users to home page
            throw redirect(302, '/')
        }
    },
}
