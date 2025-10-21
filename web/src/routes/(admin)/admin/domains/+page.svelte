<script lang="ts">
    import { enhance } from '$app/forms'
    import { page } from '$app/stores'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import {
        Card,
        CardContent,
        CardDescription,
        CardHeader,
        CardTitle,
    } from '$lib/components/ui/card'
    import { Badge } from '$lib/components/ui/badge'
    import { toast } from 'svelte-sonner'
    import type { PageData, ActionData } from './$types'

    export let data: PageData
    export let form: ActionData

    let loading = false
    let domain = ''

    // Show toast for form results
    $: if (form?.success) {
        toast.success(form.message)
    } else if (form?.error) {
        toast.error(form.error)
    }
</script>

<svelte:head>
    <title>Domain Management - Omni Admin</title>
</svelte:head>

<div class="mx-auto max-w-screen-xl space-y-6 pt-8">
    <div>
        <h1 class="text-3xl font-bold">Domain Management</h1>
        <p class="text-muted-foreground">
            Manage approved email domains for automatic user registration
        </p>
    </div>

    <div class="grid gap-6 lg:grid-cols-2">
        <!-- Add Domain Form -->
        <Card>
            <CardHeader>
                <CardTitle>Approve New Domain</CardTitle>
                <CardDescription>
                    Users with emails from approved domains can automatically register and access
                    Omni
                </CardDescription>
            </CardHeader>
            <CardContent>
                <form
                    method="POST"
                    action="?/approve"
                    class="space-y-4"
                    use:enhance={() => {
                        loading = true
                        return async ({ update }) => {
                            loading = false
                            if (form?.success) {
                                domain = ''
                            }
                            await update()
                        }
                    }}>
                    <div class="space-y-2">
                        <Label for="domain">Domain</Label>
                        <Input
                            id="domain"
                            name="domain"
                            type="text"
                            placeholder="company.com"
                            bind:value={domain}
                            disabled={loading}
                            required />
                        {#if form?.error && form?.domain}
                            <p class="text-destructive text-sm">{form.error}</p>
                        {/if}
                    </div>

                    <Button type="submit" disabled={loading || !domain.trim()}>
                        {loading ? 'Approving...' : 'Approve Domain'}
                    </Button>
                </form>
            </CardContent>
        </Card>

        <!-- Current Domains List -->
        <Card>
            <CardHeader>
                <CardTitle>Approved Domains</CardTitle>
                <CardDescription>
                    {data.domains?.length || 0} domain{(data.domains?.length || 0) !== 1 ? 's' : ''}
                    currently approved
                </CardDescription>
            </CardHeader>
            <CardContent>
                {#if data.error}
                    <div class="bg-destructive/10 rounded-md p-4">
                        <p class="text-destructive text-sm">{data.error}</p>
                    </div>
                {:else if !data.domains || data.domains.length === 0}
                    <p class="text-muted-foreground text-sm">
                        No approved domains yet. Add your first domain above.
                    </p>
                {:else}
                    <div class="space-y-3">
                        {#each data.domains as domainItem}
                            <div class="flex items-center justify-between rounded-lg border p-3">
                                <div class="flex items-center space-x-3">
                                    <Badge variant="secondary">
                                        {domainItem.domain}
                                    </Badge>
                                    <div class="text-muted-foreground text-sm">
                                        Added {new Date(domainItem.createdAt).toLocaleDateString()}
                                    </div>
                                </div>

                                <form
                                    method="POST"
                                    action="?/revoke"
                                    use:enhance={() => {
                                        return async ({ update }) => {
                                            await update()
                                        }
                                    }}>
                                    <input type="hidden" name="domain" value={domainItem.domain} />
                                    <Button variant="outline" size="sm" type="submit">
                                        Revoke
                                    </Button>
                                </form>
                            </div>
                        {/each}
                    </div>
                {/if}
            </CardContent>
        </Card>
    </div>

    <!-- Info Card -->
    <Card>
        <CardHeader>
            <CardTitle>How Domain-Based Authentication Works</CardTitle>
        </CardHeader>
        <CardContent class="space-y-3">
            <div class="text-sm">
                <h4 class="mb-2 font-medium">For Admins:</h4>
                <ul class="text-muted-foreground list-inside list-disc space-y-1">
                    <li>
                        When you create your admin account, your email domain is automatically
                        approved
                    </li>
                    <li>You can approve additional domains to allow users from other companies</li>
                    <li>
                        Users from approved domains can request magic links to sign in without
                        passwords
                    </li>
                </ul>
            </div>

            <div class="text-sm">
                <h4 class="mb-2 font-medium">For Users:</h4>
                <ul class="text-muted-foreground list-inside list-disc space-y-1">
                    <li>Users with emails from approved domains can visit the login page</li>
                    <li>They can click "Sign in with Email Link" and enter their work email</li>
                    <li>If their domain is approved, they'll receive a secure magic link</li>
                    <li>Clicking the link automatically creates their account and logs them in</li>
                </ul>
            </div>
        </CardContent>
    </Card>
</div>
