import { fail, redirect } from '@sveltejs/kit'
import { requireAuth } from '$lib/server/authHelpers'
import { hashPassword, verifyPassword } from '$lib/server/password-utils'
import { userRepository } from '$lib/server/db/users'
import type { PageServerLoad, Actions } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    const { user } = requireAuth(locals)

    return {
        mustChangePassword: user.mustChangePassword,
    }
}

export const actions: Actions = {
    default: async ({ request, locals }) => {
        const { user } = requireAuth(locals)

        const formData = await request.formData()
        const currentPassword = formData.get('currentPassword') as string
        const newPassword = formData.get('newPassword') as string
        const confirmPassword = formData.get('confirmPassword') as string

        if (!currentPassword || !newPassword || !confirmPassword) {
            return fail(400, {
                error: 'All fields are required',
                field: 'general',
            })
        }

        if (newPassword.length < 8) {
            return fail(400, {
                error: 'Password must be at least 8 characters long',
                field: 'newPassword',
            })
        }

        if (newPassword !== confirmPassword) {
            return fail(400, {
                error: 'Passwords do not match',
                field: 'confirmPassword',
            })
        }

        if (newPassword === currentPassword) {
            return fail(400, {
                error: 'New password must be different from current password',
                field: 'newPassword',
            })
        }

        try {
            const dbUser = await userRepository.findById(user.id)
            if (!dbUser || !dbUser.passwordHash) {
                return fail(400, {
                    error: 'User not found or password not set',
                    field: 'general',
                })
            }

            const validPassword = await verifyPassword(dbUser.passwordHash, currentPassword)
            if (!validPassword) {
                return fail(400, {
                    error: user.mustChangePassword
                        ? 'Temporary password is incorrect'
                        : 'Current password is incorrect',
                    field: 'currentPassword',
                })
            }

            const newPasswordHash = await hashPassword(newPassword)

            await userRepository.update(user.id, {
                passwordHash: newPasswordHash,
                mustChangePassword: false,
                updatedAt: new Date(),
            })

            return {
                success: true,
            }
        } catch (error) {
            console.error('Error changing password:', error)
            return fail(500, {
                error: 'Failed to change password. Please try again.',
                field: 'general',
            })
        }
    },
}
