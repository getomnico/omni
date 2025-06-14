<script lang="ts">
	import '../../app.css';
	import { Button } from '$lib/components/ui/button/index.js';
	import type { LayoutData } from './$types.js';

	export let data: LayoutData;

	async function logout() {
		await fetch('/logout', {
			method: 'POST'
		});
		window.location.href = '/login';
	}
</script>

<div class="min-h-screen bg-slate-50 dark:bg-slate-900">
	<!-- Header -->
	<header class="border-b border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-800">
		<div class="flex h-16 items-center justify-between px-6">
			<div class="flex items-center space-x-4">
				<h1 class="text-xl font-bold text-slate-900 dark:text-slate-100">Clio</h1>
				<nav class="hidden md:flex space-x-4">
					<a href="/" class="text-slate-600 hover:text-slate-900 dark:text-slate-300 dark:hover:text-slate-100">
						Search
					</a>
					{#if data.user.role === 'admin'}
						<a href="/admin/users" class="text-slate-600 hover:text-slate-900 dark:text-slate-300 dark:hover:text-slate-100">
							Admin
						</a>
					{/if}
				</nav>
			</div>
			
			<div class="flex items-center space-x-4">
				<span class="text-sm text-slate-600 dark:text-slate-300">
					{data.user.username}
					<span class="text-xs text-slate-500 dark:text-slate-400">({data.user.role})</span>
				</span>
				<Button variant="outline" size="sm" on:click={logout}>
					Sign out
				</Button>
			</div>
		</div>
	</header>

	<!-- Main content -->
	<main class="flex-1">
		<slot />
	</main>
</div>