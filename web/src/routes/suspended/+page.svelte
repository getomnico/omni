<script lang="ts">
    import {
        Card,
        CardContent,
        CardDescription,
        CardHeader,
        CardTitle,
    } from '$lib/components/ui/card/index.js'
    import { Button } from '$lib/components/ui/button/index.js'

    async function logout() {
        await fetch('/logout', {
            method: 'POST',
        })
        window.location.href = '/login'
    }
</script>

<svelte:head>
    <title>Account Suspended - Clio</title>
</svelte:head>

<div class="from-background to-muted min-h-screen bg-gradient-to-br">
    <div class="flex min-h-screen items-center justify-center p-4">
        <div class="w-full max-w-md">
            <div class="mb-8 text-center">
                <h1 class="text-foreground text-3xl font-bold">Clio</h1>
                <p class="text-muted-foreground mt-2">Enterprise Search Platform</p>
            </div>

            <Card class="w-full">
                <CardHeader class="text-center">
                    <CardTitle class="text-2xl">Account Suspended</CardTitle>
                    <CardDescription>Your account has been suspended</CardDescription>
                </CardHeader>
                <CardContent class="space-y-4 text-center">
                    <div class="bg-destructive/10 rounded-md p-4">
                        <div class="text-destructive text-sm">
                            Your account has been suspended by an administrator. You cannot access
                            Clio at this time.
                        </div>
                    </div>

                    <p class="text-muted-foreground text-sm">
                        Please contact your administrator for more information about your account
                        status.
                    </p>

                    <Button on:click={logout} class="w-full">Sign out</Button>
                </CardContent>
            </Card>
        </div>
    </div>
</div>
