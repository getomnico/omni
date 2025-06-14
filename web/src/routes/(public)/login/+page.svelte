<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import {
        Card,
        CardContent,
        CardDescription,
        CardHeader,
        CardTitle,
    } from '$lib/components/ui/card/index.js'
    import type { ActionData } from './$types.js'

    export let form: ActionData

    let loading = false
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
                loading = true
                return async ({ update }) => {
                    loading = false
                    await update()
                }
            }}
            class="space-y-4"
        >
            {#if form?.error}
                <div class="bg-destructive/10 rounded-md p-4">
                    <div class="text-destructive text-sm">
                        {form.error}
                    </div>
                </div>
            {/if}

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
            <span class="text-muted-foreground">Don't have an account?</span>
            <a href="/signup" class="text-foreground hover:text-foreground/80 font-medium">
                Sign up
            </a>
        </div>
    </CardContent>
</Card>
