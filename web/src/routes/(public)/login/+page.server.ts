import { fail, redirect } from '@sveltejs/kit';
import { validateSessionToken, setSessionTokenCookie } from '$lib/server/auth.js';
import { sha256 } from '@oslojs/crypto/sha2';
import { encodeHexLowerCase } from '@oslojs/encoding';
import { eq } from 'drizzle-orm';
import { db } from '$lib/server/db/index.js';
import { user, session } from '$lib/server/db/schema.js';
import { verify } from '@node-rs/argon2';
import type { Actions, PageServerLoad } from './$types.js';

// Rate limiting store (in production, use Redis)
const loginAttempts = new Map<string, { count: number; lastAttempt: number }>();
const RATE_LIMIT_MAX_ATTEMPTS = 5;
const RATE_LIMIT_WINDOW = 15 * 60 * 1000; // 15 minutes

function checkRateLimit(ip: string): boolean {
	const now = Date.now();
	const attempts = loginAttempts.get(ip);
	
	if (!attempts) {
		loginAttempts.set(ip, { count: 1, lastAttempt: now });
		return true;
	}
	
	if (now - attempts.lastAttempt > RATE_LIMIT_WINDOW) {
		loginAttempts.set(ip, { count: 1, lastAttempt: now });
		return true;
	}
	
	if (attempts.count >= RATE_LIMIT_MAX_ATTEMPTS) {
		return false;
	}
	
	attempts.count++;
	attempts.lastAttempt = now;
	return true;
}

export const load: PageServerLoad = async ({ cookies, locals }) => {
	if (locals.user) {
		throw redirect(302, '/');
	}
	return {};
};

export const actions: Actions = {
	default: async ({ request, cookies, getClientAddress }) => {
		const clientIP = getClientAddress();
		
		if (!checkRateLimit(clientIP)) {
			return fail(429, {
				error: 'Too many login attempts. Please try again in 15 minutes.'
			});
		}

		const formData = await request.formData();
		const username = formData.get('username') as string;
		const password = formData.get('password') as string;

		if (!username || !password) {
			return fail(400, {
				error: 'Username and password are required.',
				username
			});
		}

		if (typeof username !== 'string' || typeof password !== 'string') {
			return fail(400, {
				error: 'Invalid form data.',
				username
			});
		}

		if (username.length < 3 || username.length > 31) {
			return fail(400, {
				error: 'Username must be between 3 and 31 characters.',
				username
			});
		}

		if (password.length < 6 || password.length > 128) {
			return fail(400, {
				error: 'Password must be between 6 and 128 characters.',
				username
			});
		}

		try {
			const [foundUser] = await db
				.select()
				.from(user)
				.where(eq(user.username, username.toLowerCase()));

			if (!foundUser) {
				return fail(400, {
					error: 'Invalid username or password.',
					username
				});
			}

			const validPassword = await verify(foundUser.passwordHash, password);
			if (!validPassword) {
				return fail(400, {
					error: 'Invalid username or password.',
					username
				});
			}

			if (foundUser.status === 'suspended') {
				return fail(403, {
					error: 'Your account has been suspended. Please contact an administrator.',
					username
				});
			}

			if (foundUser.status === 'pending') {
				return fail(403, {
					error: 'Your account is pending approval. Please wait for an administrator to approve your account.',
					username
				});
			}

			// Create session
			const token = crypto.getRandomValues(new Uint8Array(20));
			const sessionId = encodeHexLowerCase(token);
			const sessionToken = encodeHexLowerCase(sha256(new TextEncoder().encode(sessionId)));
			
			const expiresAt = new Date();
			expiresAt.setDate(expiresAt.getDate() + 30);

			await db.insert(session).values({
				id: sessionId,
				userId: foundUser.id,
				expiresAt
			});

			setSessionTokenCookie(cookies, sessionToken, expiresAt);

			// Clear rate limiting on successful login
			loginAttempts.delete(clientIP);

		} catch (error) {
			console.error('Login error:', error);
			return fail(500, {
				error: 'An unexpected error occurred. Please try again.',
				username
			});
		}

		throw redirect(302, '/');
	}
};