<script lang="ts">
    import { enhance } from '$app/forms'
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
    let testEmail = ''

    // Show toast for form results
    $: if (form?.success) {
        toast.success(form.message)
    } else if (form?.error) {
        toast.error(form.error)
    }
</script>

<svelte:head>
    <title>Email Testing - Omni Admin</title>
</svelte:head>

<div class="mx-auto max-w-screen-xl space-y-6 pt-8">
    <div>
        <h1 class="text-3xl font-bold">Email Testing</h1>
        <p class="text-muted-foreground">
            Test your email configuration to ensure magic links are delivered properly
        </p>
    </div>

    <!-- Connection Status -->
    <Card>
        <CardHeader>
            <CardTitle>Email Provider Status</CardTitle>
            <CardDescription>Connection status with your configured email provider</CardDescription>
        </CardHeader>
        <CardContent>
            <div class="flex items-center space-x-3">
                <Badge variant={data.connectionStatus ? 'default' : 'destructive'}>
                    {data.connectionStatus ? 'Connected' : 'Connection Failed'}
                </Badge>
                <span class="text-muted-foreground text-sm">
                    {#if data.connectionStatus}
                        Email provider is properly configured and reachable
                    {:else}
                        Check your email configuration and network connectivity
                    {/if}
                </span>
            </div>
        </CardContent>
    </Card>

    <!-- Send Test Email -->
    <Card>
        <CardHeader>
            <CardTitle>Send Test Email</CardTitle>
            <CardDescription>
                Send a test magic link email to verify delivery and formatting
            </CardDescription>
        </CardHeader>
        <CardContent>
            <form
                method="POST"
                action="?/test"
                class="space-y-4"
                use:enhance={() => {
                    loading = true
                    return async ({ update }) => {
                        loading = false
                        await update()
                    }
                }}>
                <div class="space-y-2">
                    <Label for="email">Test Email Address</Label>
                    <Input
                        id="email"
                        name="email"
                        type="email"
                        placeholder="admin@yourcompany.com"
                        bind:value={testEmail}
                        disabled={loading}
                        required />
                    <p class="text-muted-foreground text-xs">
                        A test magic link will be sent to this address (the link won't work, but you
                        can verify email formatting)
                    </p>
                    {#if form?.error && form?.email}
                        <p class="text-destructive text-sm">{form.error}</p>
                    {/if}
                </div>

                <Button type="submit" disabled={loading || !testEmail.trim()}>
                    {loading ? 'Sending...' : 'Send Test Email'}
                </Button>

                {#if form?.success && form?.messageId}
                    <div class="rounded-md bg-green-50 p-4">
                        <div class="text-sm">
                            <p class="font-medium text-green-800">Test email sent successfully!</p>
                            <p class="mt-1 text-green-700">
                                Message ID: <code class="text-xs">{form.messageId}</code>
                            </p>
                        </div>
                    </div>
                {/if}
            </form>
        </CardContent>
    </Card>

    <!-- Setup Guide -->
    <Card>
        <CardHeader>
            <CardTitle>Email Configuration Guide</CardTitle>
            <CardDescription>
                How to configure email providers for your Omni instance
            </CardDescription>
        </CardHeader>
        <CardContent class="space-y-4">
            <div>
                <h4 class="mb-2 text-sm font-medium">🚀 Resend (Recommended)</h4>
                <div class="bg-muted rounded-lg p-3 font-mono text-xs">
                    <div>EMAIL_PROVIDER=resend</div>
                    <div>RESEND_API_KEY=re_abc123...</div>
                    <div>EMAIL_FROM=Omni &lt;noreply@yourdomain.com&gt;</div>
                </div>
                <p class="text-muted-foreground mt-2 text-xs">
                    Easy setup, excellent deliverability, 3,000 emails/month free
                </p>
            </div>

            <div>
                <h4 class="mb-2 text-sm font-medium">📧 SMTP (Google Workspace)</h4>
                <div class="bg-muted rounded-lg p-3 font-mono text-xs">
                    <div>EMAIL_PROVIDER=smtp</div>
                    <div>EMAIL_HOST=smtp.gmail.com</div>
                    <div>EMAIL_PORT=587</div>
                    <div>EMAIL_USER=admin@yourcompany.com</div>
                    <div>EMAIL_PASSWORD=your_app_password</div>
                    <div>EMAIL_SECURE=true</div>
                    <div>EMAIL_FROM=Omni &lt;admin@yourcompany.com&gt;</div>
                </div>
                <p class="text-muted-foreground mt-2 text-xs">
                    Requires 2FA + App Password. Limited to 2,000 emails/day per user
                </p>
            </div>

            <div class="rounded-md bg-blue-50 p-4">
                <p class="text-sm text-blue-800">
                    <strong>Pro Tip:</strong> For small-to-medium teams, Resend is significantly easier
                    to set up and has better deliverability than corporate SMTP servers.
                </p>
            </div>
        </CardContent>
    </Card>
</div>
