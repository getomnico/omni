import { fail, redirect } from '@sveltejs/kit';
import { hash } from '@node-rs/argon2';
import { eq, count } from 'drizzle-orm';
import { db } from '$lib/server/db/index.js';
import { user } from '$lib/server/db/schema.js';
import { ulid } from 'ulid';
import type { Actions, PageServerLoad } from './$types.js';

export const load: PageServerLoad = async ({ locals }) => {
	if (locals.user) {
		throw redirect(302, '/');
	}
	return {};
};

export const actions: Actions = {
	default: async ({ request }) => {
		const formData = await request.formData();
		const username = formData.get('username') as string;
		const email = formData.get('email') as string;
		const password = formData.get('password') as string;
		const confirmPassword = formData.get('confirmPassword') as string;

		// Basic validation
		if (!username || !email || !password || !confirmPassword) {
			return fail(400, {
				error: 'All fields are required.',
				username,
				email
			});
		}

		if (typeof username !== 'string' || typeof email !== 'string' || 
			typeof password !== 'string' || typeof confirmPassword !== 'string') {
			return fail(400, {
				error: 'Invalid form data.',
				username,
				email
			});
		}

		// Username validation
		if (username.length < 3 || username.length > 31) {
			return fail(400, {
				error: 'Username must be between 3 and 31 characters.',
				username,
				email
			});
		}

		if (!/^[a-zA-Z0-9_-]+$/.test(username)) {
			return fail(400, {
				error: 'Username can only contain letters, numbers, hyphens, and underscores.',
				username,
				email
			});
		}

		// Email validation
		const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
		if (!emailRegex.test(email)) {
			return fail(400, {
				error: 'Please enter a valid email address.',
				username,
				email
			});
		}

		// Password validation
		if (password.length < 8) {
			return fail(400, {
				error: 'Password must be at least 8 characters long.',
				username,
				email
			});
		}

		if (password !== confirmPassword) {
			return fail(400, {
				error: 'Passwords do not match.',
				username,
				email
			});
		}

		try {
			// Check if username already exists
			const [existingUser] = await db
				.select()
				.from(user)
				.where(eq(user.username, username.toLowerCase()));

			if (existingUser) {
				return fail(400, {
					error: 'Username is already taken.',
					username,
					email
				});
			}

			// Check if email already exists
			const [existingEmail] = await db
				.select()
				.from(user)
				.where(eq(user.email, email.toLowerCase()));

			if (existingEmail) {
				return fail(400, {
					error: 'An account with this email already exists.',
					username,
					email
				});
			}

			// Check if this is the first user
			const [userCount] = await db.select({ count: count() }).from(user);
			const isFirstUser = userCount.count === 0;

			// Hash password
			const passwordHash = await hash(password, {
				memoryCost: 65536,
				timeCost: 3,
				outputLen: 32,
				parallelism: 1
			});

			// Create user
			await db.insert(user).values({
				id: ulid(),
				username: username.toLowerCase(),
				email: email.toLowerCase(),
				passwordHash,
				role: isFirstUser ? 'admin' : 'user',
				status: isFirstUser ? 'active' : 'pending',
				createdAt: new Date(),
				approvedAt: isFirstUser ? new Date() : null
			});

			if (isFirstUser) {
				return {
					success: true,
					message: 'Account created successfully! You can now sign in as the admin.'
				};
			} else {
				return {
					success: true,
					message: 'Account created successfully! Please wait for an administrator to approve your account before signing in.'
				};
			}

		} catch (error) {
			console.error('Registration error:', error);
			return fail(500, {
				error: 'An unexpected error occurred. Please try again.',
				username,
				email
			});
		}
	}
};