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
                    <a
                        href="/login"
                        class="text-sm font-medium text-green-600 hover:text-green-500 dark:text-green-400"
                    >
                        Sign in now →
                    </a>
                </div>
            </div>
        {:else}
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
                        placeholder="Create a password"
                        required
                        disabled={loading}
                    />
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
                        disabled={loading}
                    />
                </div>

                <Button type="submit" class="w-full cursor-pointer" disabled={loading}>
                    {loading ? 'Creating account...' : 'Create account'}
                </Button>
            </form>

            <div class="mt-6 space-y-4">
                <div class="relative">
                    <div class="absolute inset-0 flex items-center">
                        <div class="border-muted w-full border-t"></div>
                    </div>
                    <div class="relative flex justify-center text-xs uppercase">
                        <span class="bg-card text-muted-foreground px-2">Or</span>
                    </div>
                </div>

                <Button
                    variant="outline"
                    class="w-full"
                    onclick="window.location.href='/auth/google'"
                >
                    <svg class="mr-2 h-4 w-4" viewBox="0 0 24 24">
                        <path
                            d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"
                            fill="#4285F4"
                        />
                        <path
                            d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"
                            fill="#34A853"
                        />
                        <path
                            d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"
                            fill="#FBBC05"
                        />
                        <path
                            d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"
                            fill="#EA4335"
                        />
                    </svg>
                    Sign up with Google
                </Button>
            </div>

            <div class="mt-6 text-center text-sm">
                <span class="text-muted-foreground">Already have an account?</span>
                <a href="/login" class="text-foreground hover:text-foreground/80 font-medium">
                    Sign in
                </a>
            </div>
        {/if}
    </CardContent>
</Card>
