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
	<title>Sign Up - Clio</title>
</svelte:head>

<Card class="w-full">
	<CardHeader class="text-center">
		<CardTitle class="text-2xl">Create your account</CardTitle>
		<CardDescription>Get started with Clio Enterprise Search</CardDescription>
	</CardHeader>
	<CardContent>
		{#if form?.success}
			<div class="rounded-md bg-green-50 p-4 dark:bg-green-900/50">
				<div class="text-sm text-green-800 dark:text-green-200">
					{form.message}
				</div>
				<div class="mt-3">
					<a href="/login" class="text-sm font-medium text-green-600 hover:text-green-500 dark:text-green-400">
						Sign in now →
					</a>
				</div>
			</div>
		{:else}
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
						placeholder="Choose a username"
						value={form?.username ?? ''}
						required
						disabled={loading}
					/>
					<p class="text-xs text-slate-500 dark:text-slate-400">
						3-31 characters, letters, numbers, hyphens, and underscores only
					</p>
				</div>

				<div class="space-y-2">
					<Label for="email">Email</Label>
					<Input
						id="email"
						name="email"
						type="email"
						placeholder="Enter your email"
						value={form?.email ?? ''}
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
						placeholder="Create a password"
						required
						disabled={loading}
					/>
					<p class="text-xs text-slate-500 dark:text-slate-400">
						At least 8 characters
					</p>
				</div>

				<div class="space-y-2">
					<Label for="confirmPassword">Confirm Password</Label>
					<Input
						id="confirmPassword"
						name="confirmPassword"
						type="password"
						placeholder="Confirm your password"
						required
						disabled={loading}
					/>
				</div>

				<Button type="submit" class="w-full" disabled={loading}>
					{loading ? 'Creating account...' : 'Create account'}
				</Button>
			</form>

			<div class="mt-6 text-center text-sm">
				<span class="text-slate-600 dark:text-slate-400">Already have an account?</span>
				<a href="/login" class="font-medium text-slate-900 hover:text-slate-700 dark:text-slate-100 dark:hover:text-slate-300">
					Sign in
				</a>
			</div>
		{/if}
	</CardContent>
</Card>