<script lang="ts">
	import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '$lib/components/ui/card/index.js';
	import { Button } from '$lib/components/ui/button/index.js';

	async function logout() {
		await fetch('/logout', {
			method: 'POST'
		});
		window.location.href = '/login';
	}
</script>

<svelte:head>
	<title>Account Suspended - Clio</title>
</svelte:head>

<div class="min-h-screen bg-gradient-to-br from-slate-50 to-slate-100 dark:from-slate-900 dark:to-slate-800">
	<div class="flex min-h-screen items-center justify-center p-4">
		<div class="w-full max-w-md">
			<div class="mb-8 text-center">
				<h1 class="text-3xl font-bold text-slate-900 dark:text-slate-100">Clio</h1>
				<p class="mt-2 text-slate-600 dark:text-slate-400">Enterprise Search Platform</p>
			</div>
			
			<Card class="w-full">
				<CardHeader class="text-center">
					<CardTitle class="text-2xl">Account Suspended</CardTitle>
					<CardDescription>Your account has been suspended</CardDescription>
				</CardHeader>
				<CardContent class="text-center space-y-4">
					<div class="rounded-md bg-red-50 p-4 dark:bg-red-900/50">
						<div class="text-sm text-red-800 dark:text-red-200">
							Your account has been suspended by an administrator. You cannot access Clio at this time.
						</div>
					</div>
					
					<p class="text-sm text-slate-600 dark:text-slate-400">
						Please contact your administrator for more information about your account status.
					</p>
					
					<Button on:click={logout} class="w-full">
						Sign out
					</Button>
				</CardContent>
			</Card>
		</div>
	</div>
</div>