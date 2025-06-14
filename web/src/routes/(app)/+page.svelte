<script lang="ts">
	import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import type { PageData } from './$types.js';

	export let data: PageData;
</script>

<svelte:head>
	<title>Clio - Enterprise Search</title>
</svelte:head>

<div class="container mx-auto px-4 py-8">
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-slate-900 dark:text-slate-100">Welcome to Clio</h1>
		<p class="mt-2 text-slate-600 dark:text-slate-400">
			Your unified enterprise search platform. Search across all your connected data sources.
		</p>
	</div>

	<!-- Search Box -->
	<div class="mb-8">
		<Card>
			<CardContent class="p-6">
				<div class="space-y-4">
					<input
						type="text"
						placeholder="Search across all your data sources..."
						class="w-full rounded-md border border-slate-300 px-4 py-3 text-lg focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-500/20 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
					/>
					<Button class="w-full sm:w-auto">
						Search
					</Button>
				</div>
			</CardContent>
		</Card>
	</div>

	<!-- Quick Stats -->
	<div class="grid grid-cols-1 gap-6 md:grid-cols-3">
		<Card>
			<CardHeader>
				<CardTitle class="text-lg">Connected Sources</CardTitle>
			</CardHeader>
			<CardContent>
				<div class="text-2xl font-bold text-slate-900 dark:text-slate-100">0</div>
				<p class="text-sm text-slate-600 dark:text-slate-400">Data sources connected</p>
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle class="text-lg">Indexed Documents</CardTitle>
			</CardHeader>
			<CardContent>
				<div class="text-2xl font-bold text-slate-900 dark:text-slate-100">0</div>
				<p class="text-sm text-slate-600 dark:text-slate-400">Documents ready to search</p>
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle class="text-lg">Recent Searches</CardTitle>
			</CardHeader>
			<CardContent>
				<div class="text-2xl font-bold text-slate-900 dark:text-slate-100">0</div>
				<p class="text-sm text-slate-600 dark:text-slate-400">Searches performed today</p>
			</CardContent>
		</Card>
	</div>

	{#if data.user.role === 'admin'}
		<div class="mt-8">
			<Card>
				<CardHeader>
					<CardTitle>Admin Quick Actions</CardTitle>
				</CardHeader>
				<CardContent class="space-y-4">
					<div class="flex flex-wrap gap-4">
						<a href="/admin/users">
							<Button variant="outline">
								Manage Users
							</Button>
						</a>
						<Button variant="outline">
							Connect Data Sources
						</Button>
						<Button variant="outline">
							View Analytics
						</Button>
					</div>
				</CardContent>
			</Card>
		</div>
	{/if}
</div>