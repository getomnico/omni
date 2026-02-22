<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { CheckCircle2, Loader2, Info, Pencil } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageData } from './$types'
    import googleIcon from '$lib/images/icons/google.svg'

    let { data }: { data: PageData } = $props()

    let enabled = $state(data.google.enabled)
    let clientId = $state(data.google.clientId)
    let clientSecret = $state('')
    let isSubmitting = $state(false)
    let showForm = $state(false)

    function handleToggle() {
        if (enabled) {
            showForm = false
        } else {
            showForm = true
        }
        enabled = !enabled
    }
</script>

<svelte:head>
    <title>Authentication - Settings - Omni</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Authentication</h1>
            <p class="text-muted-foreground mt-2">
                Configure how users sign in to your Omni instance
            </p>
        </div>

        <div class="grid grid-cols-1 items-start gap-4 md:grid-cols-2">
            <Card.Root>
                <Card.Header class="flex flex-row items-start justify-between space-y-0 pb-2">
                    <div class="flex items-start gap-3">
                        <img src={googleIcon} alt="Google" class="h-8 w-8" />
                        <div>
                            <Card.Title class="text-lg">Google</Card.Title>
                            {#if data.google.enabled}
                                <div class="flex items-center gap-1.5 text-sm text-green-600">
                                    <CheckCircle2 class="h-3.5 w-3.5" />
                                    Enabled
                                </div>
                            {:else}
                                <Card.Description>Sign in with Google Workspace</Card.Description>
                            {/if}
                        </div>
                    </div>
                </Card.Header>
                <Card.Content>
                    {#if data.google.enabled && !showForm}
                        <div class="flex flex-wrap gap-2">
                            <Button
                                variant="outline"
                                size="sm"
                                class="cursor-pointer gap-1"
                                onclick={() => (showForm = true)}>
                                <Pencil class="h-3 w-3" />
                                Edit
                            </Button>
                            <form
                                method="POST"
                                action="?/update"
                                use:enhance={() => {
                                    isSubmitting = true
                                    return async ({ result, update }) => {
                                        isSubmitting = false
                                        await update()
                                        if (result.type === 'success') {
                                            toast.success(
                                                result.data?.message || 'Google Auth disabled',
                                            )
                                            enabled = false
                                        } else if (result.type === 'failure') {
                                            toast.error(
                                                result.data?.error || 'Something went wrong',
                                            )
                                        }
                                    }
                                }}>
                                <input type="hidden" name="enabled" value="false" />
                                <input type="hidden" name="clientId" value="" />
                                <input type="hidden" name="clientSecret" value="" />
                                <Button
                                    type="submit"
                                    variant="outline"
                                    size="sm"
                                    disabled={isSubmitting}
                                    class="cursor-pointer gap-1 text-red-600 hover:text-red-700">
                                    {isSubmitting ? 'Disabling...' : 'Disable'}
                                </Button>
                            </form>
                        </div>
                    {:else if showForm || !data.google.enabled}
                        <form
                            method="POST"
                            action="?/update"
                            use:enhance={() => {
                                isSubmitting = true
                                return async ({ result, update }) => {
                                    isSubmitting = false
                                    await update()
                                    if (result.type === 'success') {
                                        toast.success(result.data?.message || 'Settings saved')
                                        clientSecret = ''
                                        showForm = false
                                    } else if (result.type === 'failure') {
                                        toast.error(result.data?.error || 'Something went wrong')
                                    }
                                }
                            }}
                            class="space-y-4">
                            <input type="hidden" name="enabled" value="true" />

                            <Alert.Root>
                                <Info class="h-4 w-4" />
                                <Alert.Description>
                                    Create a Google Cloud project, configure the OAuth consent
                                    screen as Internal, create OAuth 2.0 credentials, and paste them
                                    here. Set the authorized redirect URI to
                                    <code class="bg-muted rounded px-1 text-sm"
                                        >{'{app_url}'}/auth/google/callback</code>
                                </Alert.Description>
                            </Alert.Root>

                            <div class="space-y-2">
                                <Label for="clientId">Client ID *</Label>
                                <Input
                                    id="clientId"
                                    name="clientId"
                                    bind:value={clientId}
                                    placeholder="123456789.apps.googleusercontent.com"
                                    required />
                            </div>

                            <div class="space-y-2">
                                <Label for="clientSecret">
                                    Client Secret {data.google.hasClientSecret ? '' : '*'}
                                </Label>
                                <Input
                                    id="clientSecret"
                                    name="clientSecret"
                                    type="password"
                                    bind:value={clientSecret}
                                    placeholder={data.google.hasClientSecret
                                        ? 'Leave empty to keep current secret'
                                        : 'GOCSPX-...'}
                                    required={!data.google.hasClientSecret} />
                            </div>

                            <div class="flex gap-2">
                                {#if data.google.enabled}
                                    <Button
                                        variant="outline"
                                        type="button"
                                        class="cursor-pointer"
                                        onclick={() => (showForm = false)}>
                                        Cancel
                                    </Button>
                                {/if}
                                <Button
                                    type="submit"
                                    disabled={isSubmitting}
                                    class="cursor-pointer">
                                    {#if isSubmitting}
                                        <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                                        Saving...
                                    {:else}
                                        {data.google.enabled ? 'Update' : 'Enable'}
                                    {/if}
                                </Button>
                            </div>
                        </form>
                    {/if}
                </Card.Content>
            </Card.Root>
        </div>
    </div>
</div>
