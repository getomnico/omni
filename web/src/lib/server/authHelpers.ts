import { redirect } from '@sveltejs/kit';
import type { SessionValidationResult } from './auth';

export function requireAuth(locals: App.Locals, redirectTo = '/auth/login') {
	if (!locals.user || !locals.session) {
		throw redirect(302, redirectTo);
	}
	return { user: locals.user, session: locals.session };
}

export function requireRole(locals: App.Locals, requiredRole: 'admin' | 'user' | 'viewer', redirectTo = '/') {
	const { user } = requireAuth(locals);
	
	const roleHierarchy = { admin: 3, user: 2, viewer: 1 };
	const userLevel = roleHierarchy[user.role as keyof typeof roleHierarchy] || 0;
	const requiredLevel = roleHierarchy[requiredRole];
	
	if (userLevel < requiredLevel) {
		throw redirect(302, redirectTo);
	}
	
	return { user, session: locals.session! };
}

export function requireAdmin(locals: App.Locals, redirectTo = '/') {
	return requireRole(locals, 'admin', redirectTo);
}

export function requireActiveUser(locals: App.Locals, redirectTo = '/auth/login') {
	const { user, session } = requireAuth(locals, redirectTo);
	
	if (user.status !== 'active') {
		throw redirect(302, '/auth/login?error=account-not-active');
	}
	
	return { user, session };
}