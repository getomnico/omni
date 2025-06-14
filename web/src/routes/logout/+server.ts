import { redirect } from '@sveltejs/kit';
import { deleteSessionTokenCookie } from '$lib/server/auth.js';
import { db } from '$lib/server/db/index.js';
import { session } from '$lib/server/db/schema.js';
import { eq } from 'drizzle-orm';
import type { RequestHandler } from './$types.js';

export const POST: RequestHandler = async ({ cookies, locals }) => {
	if (locals.session) {
		await db.delete(session).where(eq(session.id, locals.session.id));
	}
	
	deleteSessionTokenCookie(cookies);
	throw redirect(302, '/login');
};