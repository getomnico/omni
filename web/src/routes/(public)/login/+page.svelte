<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '$lib/components/ui/card/index.js';
	import type { ActionData } from './$types.js';

	export let form: ActionData;

	let loading = false;
</script>

<svelte:head>
	<title>Login - Clio</title>
</svelte:head>

<Card class="w-full">
	<CardHeader class="text-center">
		<CardTitle class="text-2xl">Welcome back</CardTitle>
		<CardDescription>Sign in to your Clio account</CardDescription>
	</CardHeader>
	<CardContent>
		<form 
			method="POST" 
			use:enhance={() => {
				loading = true;
				return async ({ update }) => {
					loading = false;
					await update();
				};
			}}
			class="space-y-4"
		>
			{#if form?.error}
				<div class="rounded-md bg-red-50 p-4 dark:bg-red-900/50">
					<div class="text-sm text-red-800 dark:text-red-200">
						{form.error}
					</div>
				</div>
			{/if}

			<div class="space-y-2">
				<Label for="username">Username</Label>
				<Input
					id="username"
					name="username"
					type="text"
					placeholder="Enter your username"
					value={form?.username ?? ''}
					required
					disabled={loading}
				/>
			</div>

			<div class="space-y-2">
				<Label for="password">Password</Label>
				<Input
					id="password"
					name="password"
					type="password"
					placeholder="Enter your password"
					required
					disabled={loading}
				/>
			</div>

			<Button type="submit" class="w-full" disabled={loading}>
				{loading ? 'Signing in...' : 'Sign in'}
			</Button>
		</form>

		<div class="mt-6 text-center text-sm">
			<span class="text-slate-600 dark:text-slate-400">Don't have an account?</span>
			<a href="/signup" class="font-medium text-slate-900 hover:text-slate-700 dark:text-slate-100 dark:hover:text-slate-300">
				Sign up
			</a>
		</div>
	</CardContent>
</Card>