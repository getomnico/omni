<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import { type PageProps } from './$types'
    import {
        Card,
        CardContent,
        CardDescription,
        CardHeader,
        CardTitle,
    } from '$lib/components/ui/card/index.js'

    const { data, form }: PageProps = $props()

    let loading = $state(false)
</script>

<svelte:head>
    <title>{data.isFirstUser ? 'Welcome to Omni' : 'Sign Up'}</title>
</svelte:head>

<Card class="w-full">
    <CardHeader class="text-center">
        {#if data.isFirstUser}
            <CardTitle class="text-xl">Create admin account to get started</CardTitle>
            <CardDescription>Create an admin account to get started.</CardDescription>
        {:else}
            <CardTitle class="text-2xl">Create your account</CardTitle>
            <CardDescription>Get started with Omni Enterprise Search</CardDescription>
        {/if}
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
            class="space-y-4">
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
                    disabled={loading} />
            </div>

            <div class="space-y-2">
                <Label for="password">Password</Label>
                <Input
                    id="password"
                    name="password"
                    type="password"
                    placeholder="Create a password"
                    required
                    disabled={loading} />
                <p class="text-muted-foreground text-xs">At least 8 characters</p>
            </div>

            <div class="space-y-2">
                <Label for="confirmPassword">Confirm Password</Label>
                <Input
                    id="confirmPassword"
                    name="confirmPassword"
                    type="password"
                    placeholder="Confirm your password"
                    required
                    disabled={loading} />
            </div>

            <Button type="submit" class="w-full cursor-pointer" disabled={loading}>
                {#if loading}
                    {data.isFirstUser ? 'Creating admin account...' : 'Creating account...'}
                {:else}
                    {data.isFirstUser ? 'Create admin account' : 'Create account'}
                {/if}
            </Button>
        </form>

        <div class="mt-6 text-center text-sm">
            <span class="text-muted-foreground">Already have an account?</span>
            <a href="/login" class="text-foreground hover:text-foreground/80 font-medium">
                Sign in
            </a>
        </div>
    </CardContent>
</Card>
