import { fail } from '@sveltejs/kit';
import { eq } from 'drizzle-orm';
import { db } from '$lib/server/db';
import * as table from '$lib/server/db/schema';
import { requireAdmin } from '$lib/server/authHelpers';
import type { PageServerLoad, Actions } from './$types';

export const load: PageServerLoad = async ({ locals }) => {
	requireAdmin(locals);

	// Get all users
	const users = await db
		.select({
			id: table.user.id,
			username: table.user.username,
			email: table.user.email,
			role: table.user.role,
			status: table.user.status,
			createdAt: table.user.createdAt,
			approvedAt: table.user.approvedAt
		})
		.from(table.user)
		.orderBy(table.user.createdAt);

	return {
		users
	};
};

export const actions: Actions = {
	approve: async ({ request, locals }) => {
		const { user } = requireAdmin(locals);

		const formData = await request.formData();
		const userId = formData.get('userId') as string;

		if (!userId) {
			return fail(400, { message: 'User ID required' });
		}

		try {
			await db
				.update(table.user)
				.set({
					status: 'active',
					approvedAt: new Date(),
					approvedBy: user.id
				})
				.where(eq(table.user.id, userId));

			return { success: true };
		} catch (error) {
			return fail(500, { message: 'Failed to approve user' });
		}
	},

	suspend: async ({ request, locals }) => {
		const { user } = requireAdmin(locals);

		const formData = await request.formData();
		const userId = formData.get('userId') as string;

		if (!userId) {
			return fail(400, { message: 'User ID required' });
		}

		// Don't allow admins to suspend themselves
		if (userId === user.id) {
			return fail(400, { message: 'Cannot suspend your own account' });
		}

		try {
			await db
				.update(table.user)
				.set({ status: 'suspended' })
				.where(eq(table.user.id, userId));

			return { success: true };
		} catch (error) {
			return fail(500, { message: 'Failed to suspend user' });
		}
	},

	activate: async ({ request, locals }) => {
		const { user } = requireAdmin(locals);

		const formData = await request.formData();
		const userId = formData.get('userId') as string;

		if (!userId) {
			return fail(400, { message: 'User ID required' });
		}

		try {
			await db
				.update(table.user)
				.set({ status: 'active' })
				.where(eq(table.user.id, userId));

			return { success: true };
		} catch (error) {
			return fail(500, { message: 'Failed to activate user' });
		}
	},

	updateRole: async ({ request, locals }) => {
		const { user } = requireAdmin(locals);

		const formData = await request.formData();
		const userId = formData.get('userId') as string;
		const role = formData.get('role') as string;

		if (!userId || !role) {
			return fail(400, { message: 'User ID and role required' });
		}

		if (!['admin', 'user', 'viewer'].includes(role)) {
			return fail(400, { message: 'Invalid role' });
		}

		// Don't allow the last admin to be demoted
		if (userId === user.id && role !== 'admin') {
			const adminCount = await db
				.select()
				.from(table.user)
				.where(eq(table.user.role, 'admin'));

			if (adminCount.length === 1) {
				return fail(400, { message: 'Cannot remove the last admin' });
			}
		}

		try {
			await db
				.update(table.user)
				.set({ role })
				.where(eq(table.user.id, userId));

			return { success: true };
		} catch (error) {
			return fail(500, { message: 'Failed to update role' });
		}
	}
};