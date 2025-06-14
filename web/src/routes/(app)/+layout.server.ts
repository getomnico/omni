import { redirect } from '@sveltejs/kit';
import type { LayoutServerLoad } from './$types.js';

export const load: LayoutServerLoad = async ({ locals }) => {
	if (!locals.user) {
		throw redirect(302, '/login');
	}

	if (locals.user.status === 'pending') {
		throw redirect(302, '/pending');
	}

	if (locals.user.status === 'suspended') {
		throw redirect(302, '/suspended'); 
	}

	return {
		user: locals.user
	};
};