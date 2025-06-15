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
    <title>Account Pending - Clio</title>
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
                    <CardTitle class="text-2xl">Account Pending</CardTitle>
                    <CardDescription>Your account is awaiting approval</CardDescription>
                </CardHeader>
                <CardContent class="space-y-4 text-center">
                    <div class="rounded-md bg-yellow-50 p-4 dark:bg-yellow-900/50">
                        <div class="text-sm text-yellow-800 dark:text-yellow-200">
                            Your account has been created successfully, but it needs to be approved
                            by an administrator before you can access Clio.
                        </div>
                    </div>

                    <p class="text-muted-foreground text-sm">
                        You will receive an email notification once your account has been approved.
                        Please contact your administrator if you have any questions.
                    </p>

                    <Button onclick={logout} class="w-full">Sign out</Button>
                </CardContent>
            </Card>
        </div>
    </div>
</div>
