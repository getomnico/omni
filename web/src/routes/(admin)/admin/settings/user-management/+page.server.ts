import { fail } from '@sveltejs/kit'
import { eq, or, like, count } from 'drizzle-orm'
import { ulid } from 'ulid'
import { db } from '$lib/server/db'
import * as table from '$lib/server/db/schema'
import { requireAdmin } from '$lib/server/authHelpers'
import { hashPassword, generateSecurePassword } from '$lib/server/password-utils'
import { userRepository } from '$lib/server/db/users'
import { DomainService } from '$lib/server/domains'
import type { PageServerLoad, Actions } from './$types'

export const load: PageServerLoad = async ({ locals, url }) => {
    requireAdmin(locals)

    const searchQuery = url.searchParams.get('search') || ''

    let usersQuery = db
        .select({
            id: table.user.id,
            email: table.user.email,
            role: table.user.role,
            isActive: table.user.isActive,
            mustChangePassword: table.user.mustChangePassword,
            createdAt: table.user.createdAt,
            updatedAt: table.user.updatedAt,
        })
        .from(table.user)

    if (searchQuery) {
        usersQuery = usersQuery.where(or(like(table.user.email, `%${searchQuery}%`)))
    }

    const users = await usersQuery.orderBy(table.user.createdAt)

    return {
        users,
        searchQuery,
    }
}

export const actions: Actions = {
    createUser: async ({ request, locals }) => {
        const { user: currentUser } = requireAdmin(locals)

        const formData = await request.formData()
        const email = formData.get('email') as string
        const role = (formData.get('role') as string) || 'user'

        if (!email) {
            return fail(400, { error: 'Email is required', field: 'email' })
        }

        const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/
        if (!emailRegex.test(email)) {
            return fail(400, { error: 'Invalid email address', field: 'email' })
        }

        if (!['admin', 'user', 'viewer'].includes(role)) {
            return fail(400, { error: 'Invalid role', field: 'role' })
        }

        try {
            const emailExists = await userRepository.emailExists(email)
            if (emailExists) {
                return fail(400, {
                    error: 'A user with this email already exists',
                    field: 'email',
                })
            }

            const password = generateSecurePassword()
            const passwordHash = await hashPassword(password)
            const domain = DomainService.extractDomainFromEmail(email.toLowerCase())

            const newUserId = ulid()
            await userRepository.create({
                id: newUserId,
                email: email.toLowerCase(),
                passwordHash,
                role,
                authMethod: 'password',
                domain,
                isActive: true,
                mustChangePassword: true,
                createdAt: new Date(),
            })

            return {
                success: true,
                action: 'createUser',
                password,
                email: email.toLowerCase(),
            }
        } catch (error) {
            console.error('Error creating user:', error)
            return fail(500, { error: 'Failed to create user', field: 'general' })
        }
    },

    resetPassword: async ({ request, locals }) => {
        const { user: currentUser } = requireAdmin(locals)

        const formData = await request.formData()
        const userId = formData.get('userId') as string

        if (!userId) {
            return fail(400, { error: 'User ID is required', field: 'general' })
        }

        if (userId === currentUser.id) {
            return fail(400, {
                error: 'Use Change Password from your profile to update your own password',
                field: 'general',
            })
        }

        try {
            const targetUser = await userRepository.findById(userId)
            if (!targetUser) {
                return fail(404, { error: 'User not found', field: 'general' })
            }

            const newPassword = generateSecurePassword()
            const passwordHash = await hashPassword(newPassword)

            await userRepository.update(userId, {
                passwordHash,
                mustChangePassword: true,
                updatedAt: new Date(),
            })

            return {
                success: true,
                action: 'resetPassword',
                password: newPassword,
                email: targetUser.email,
            }
        } catch (error) {
            console.error('Error resetting password:', error)
            return fail(500, { error: 'Failed to reset password', field: 'general' })
        }
    },

    updateUser: async ({ request, locals }) => {
        const { user: currentUser } = requireAdmin(locals)

        const formData = await request.formData()
        const userId = formData.get('userId') as string
        const email = formData.get('email') as string
        const role = formData.get('role') as string

        if (!userId) {
            return fail(400, { error: 'User ID is required', field: 'general' })
        }

        if (!email) {
            return fail(400, { error: 'Email is required', field: 'email' })
        }

        const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/
        if (!emailRegex.test(email)) {
            return fail(400, { error: 'Invalid email address', field: 'email' })
        }

        if (!['admin', 'user', 'viewer'].includes(role)) {
            return fail(400, { error: 'Invalid role', field: 'role' })
        }

        try {
            const targetUser = await userRepository.findById(userId)
            if (!targetUser) {
                return fail(404, { error: 'User not found', field: 'general' })
            }

            if (email.toLowerCase() !== targetUser.email) {
                const emailExists = await userRepository.emailExists(email)
                if (emailExists) {
                    return fail(400, {
                        error: 'A user with this email already exists',
                        field: 'email',
                    })
                }
            }

            if (role !== targetUser.role && userId === currentUser.id && role !== 'admin') {
                const [adminCountResult] = await db
                    .select({ count: count() })
                    .from(table.user)
                    .where(eq(table.user.role, 'admin'))

                if (adminCountResult.count === 1) {
                    return fail(400, {
                        error: 'Cannot demote the last admin',
                        field: 'role',
                    })
                }
            }

            const domain = DomainService.extractDomainFromEmail(email.toLowerCase())

            await userRepository.update(userId, {
                email: email.toLowerCase(),
                role,
                domain,
                updatedAt: new Date(),
            })

            return {
                success: true,
                action: 'updateUser',
                message: 'User updated successfully',
            }
        } catch (error) {
            console.error('Error updating user:', error)
            return fail(500, { error: 'Failed to update user', field: 'general' })
        }
    },

    deleteUser: async ({ request, locals }) => {
        const { user: currentUser } = requireAdmin(locals)

        const formData = await request.formData()
        const userId = formData.get('userId') as string

        if (!userId) {
            return fail(400, { error: 'User ID is required', field: 'general' })
        }

        if (userId === currentUser.id) {
            return fail(400, {
                error: 'Cannot delete your own account',
                field: 'general',
            })
        }

        try {
            const targetUser = await userRepository.findById(userId)
            if (!targetUser) {
                return fail(404, { error: 'User not found', field: 'general' })
            }

            if (targetUser.role === 'admin') {
                const [adminCountResult] = await db
                    .select({ count: count() })
                    .from(table.user)
                    .where(eq(table.user.role, 'admin'))

                if (adminCountResult.count === 1) {
                    return fail(400, {
                        error: 'Cannot delete the last admin',
                        field: 'general',
                    })
                }
            }

            await db.delete(table.user).where(eq(table.user.id, userId))

            return {
                success: true,
                action: 'deleteUser',
                message: 'User deleted successfully',
            }
        } catch (error) {
            console.error('Error deleting user:', error)
            return fail(500, { error: 'Failed to delete user', field: 'general' })
        }
    },

    toggleActive: async ({ request, locals }) => {
        const { user: currentUser } = requireAdmin(locals)

        const formData = await request.formData()
        const userId = formData.get('userId') as string

        if (!userId) {
            return fail(400, { error: 'User ID is required', field: 'general' })
        }

        if (userId === currentUser.id) {
            return fail(400, {
                error: 'Cannot deactivate your own account',
                field: 'general',
            })
        }

        try {
            const targetUser = await userRepository.findById(userId)
            if (!targetUser) {
                return fail(404, { error: 'User not found', field: 'general' })
            }

            await userRepository.update(userId, {
                isActive: !targetUser.isActive,
                updatedAt: new Date(),
            })

            return {
                success: true,
                action: 'toggleActive',
                message: `User ${targetUser.isActive ? 'deactivated' : 'activated'} successfully`,
            }
        } catch (error) {
            console.error('Error toggling user status:', error)
            return fail(500, {
                error: 'Failed to update user status',
                field: 'general',
            })
        }
    },
}
