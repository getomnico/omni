<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { CheckCircle2, Loader2, Info, Pencil, KeyRound } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageData } from './$types'
    import googleIcon from '$lib/images/icons/google.svg'
    import oktaIcon from '$lib/images/icons/okta.svg'

    let { data }: { data: PageData } = $props()

    let enabled = $state(data.google.enabled)
    let clientId = $state(data.google.clientId)
    let clientSecret = $state('')
    let isSubmitting = $state(false)
    let showForm = $state(false)

    let oktaEnabled = $state(data.okta.enabled)
    let oktaDomain = $state(data.okta.oktaDomain)
    let oktaClientId = $state(data.okta.clientId)
    let oktaClientSecret = $state('')
    let oktaIsSubmitting = $state(false)
    let oktaShowForm = $state(false)

    let passwordEnabled = $state(data.passwordAuthEnabled)
    let passwordIsSubmitting = $state(false)

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
                        <KeyRound class="text-muted-foreground h-8 w-8" />
                        <div>
                            <Card.Title class="text-lg">Password</Card.Title>
                            {#if passwordEnabled}
                                <div class="flex items-center gap-1.5 text-sm text-green-600">
                                    <CheckCircle2 class="h-3.5 w-3.5" />
                                    Enabled
                                </div>
                            {:else}
                                <Card.Description
                                    >Email and password authentication is disabled</Card.Description>
                            {/if}
                        </div>
                    </div>
                </Card.Header>
                <Card.Content>
                    <form
                        class="flex justify-end"
                        method="POST"
                        action="?/updatePassword"
                        use:enhance={() => {
                            passwordIsSubmitting = true
                            return async ({ result, update }) => {
                                passwordIsSubmitting = false
                                await update()
                                if (result.type === 'success') {
                                    toast.success(result.data?.message || 'Settings saved')
                                    passwordEnabled = !passwordEnabled
                                } else if (result.type === 'failure') {
                                    toast.error(result.data?.error || 'Something went wrong')
                                }
                            }
                        }}>
                        <input type="hidden" name="enabled" value={!passwordEnabled} />
                        {#if passwordEnabled}
                            <Button
                                type="submit"
                                variant="outline"
                                size="sm"
                                disabled={passwordIsSubmitting}
                                class="cursor-pointer gap-1 text-red-600 hover:text-red-700">
                                {passwordIsSubmitting ? 'Disabling...' : 'Disable'}
                            </Button>
                        {:else}
                            <Button
                                type="submit"
                                size="sm"
                                disabled={passwordIsSubmitting}
                                class="cursor-pointer">
                                {passwordIsSubmitting ? 'Enabling...' : 'Enable'}
                            </Button>
                        {/if}
                    </form>
                </Card.Content>
            </Card.Root>

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
                        <div class="flex flex-wrap justify-end gap-2">
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

                            <div class="flex justify-end gap-2">
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

            {#if data.oktaSsoAvailable}
                <Card.Root>
                    <Card.Header class="flex flex-row items-start justify-between space-y-0 pb-2">
                        <div class="flex items-start gap-3">
                            <img src={oktaIcon} alt="Okta" class="h-8 w-8" />
                            <div>
                                <Card.Title class="text-lg">Okta</Card.Title>
                                {#if data.okta.enabled}
                                    <div class="flex items-center gap-1.5 text-sm text-green-600">
                                        <CheckCircle2 class="h-3.5 w-3.5" />
                                        Enabled
                                    </div>
                                {:else}
                                    <Card.Description>Sign in with Okta SSO</Card.Description>
                                {/if}
                            </div>
                        </div>
                    </Card.Header>
                    <Card.Content>
                        {#if data.okta.enabled && !oktaShowForm}
                            <div class="flex flex-wrap justify-end gap-2">
                                <Button
                                    variant="outline"
                                    size="sm"
                                    class="cursor-pointer gap-1"
                                    onclick={() => (oktaShowForm = true)}>
                                    <Pencil class="h-3 w-3" />
                                    Edit
                                </Button>
                                <form
                                    method="POST"
                                    action="?/updateOkta"
                                    use:enhance={() => {
                                        oktaIsSubmitting = true
                                        return async ({ result, update }) => {
                                            oktaIsSubmitting = false
                                            await update()
                                            if (result.type === 'success') {
                                                toast.success(
                                                    result.data?.message || 'Okta SSO disabled',
                                                )
                                                oktaEnabled = false
                                            } else if (result.type === 'failure') {
                                                toast.error(
                                                    result.data?.error || 'Something went wrong',
                                                )
                                            }
                                        }
                                    }}>
                                    <input type="hidden" name="enabled" value="false" />
                                    <input type="hidden" name="oktaDomain" value="" />
                                    <input type="hidden" name="clientId" value="" />
                                    <input type="hidden" name="clientSecret" value="" />
                                    <Button
                                        type="submit"
                                        variant="outline"
                                        size="sm"
                                        disabled={oktaIsSubmitting}
                                        class="cursor-pointer gap-1 text-red-600 hover:text-red-700">
                                        {oktaIsSubmitting ? 'Disabling...' : 'Disable'}
                                    </Button>
                                </form>
                            </div>
                        {:else if oktaShowForm || !data.okta.enabled}
                            <form
                                method="POST"
                                action="?/updateOkta"
                                use:enhance={() => {
                                    oktaIsSubmitting = true
                                    return async ({ result, update }) => {
                                        oktaIsSubmitting = false
                                        await update()
                                        if (result.type === 'success') {
                                            toast.success(result.data?.message || 'Settings saved')
                                            oktaClientSecret = ''
                                            oktaShowForm = false
                                        } else if (result.type === 'failure') {
                                            toast.error(
                                                result.data?.error || 'Something went wrong',
                                            )
                                        }
                                    }
                                }}
                                class="space-y-4">
                                <input type="hidden" name="enabled" value="true" />

                                <Alert.Root>
                                    <Info class="h-4 w-4" />
                                    <Alert.Description>
                                        Create an Okta application (Web, OIDC), and paste the
                                        credentials here. Set the sign-in redirect URI to
                                        <code class="bg-muted rounded px-1 text-sm"
                                            >{'{app_url}'}/auth/okta/callback</code>
                                    </Alert.Description>
                                </Alert.Root>

                                <div class="space-y-2">
                                    <Label for="oktaDomain">Okta Domain *</Label>
                                    <Input
                                        id="oktaDomain"
                                        name="oktaDomain"
                                        bind:value={oktaDomain}
                                        placeholder="mycompany.okta.com"
                                        required />
                                </div>

                                <div class="space-y-2">
                                    <Label for="oktaClientId">Client ID *</Label>
                                    <Input
                                        id="oktaClientId"
                                        name="clientId"
                                        bind:value={oktaClientId}
                                        placeholder="0oa1b2c3d4e5f6g7h8i9"
                                        required />
                                </div>

                                <div class="space-y-2">
                                    <Label for="oktaClientSecret">
                                        Client Secret {data.okta.hasClientSecret ? '' : '*'}
                                    </Label>
                                    <Input
                                        id="oktaClientSecret"
                                        name="clientSecret"
                                        type="password"
                                        bind:value={oktaClientSecret}
                                        placeholder={data.okta.hasClientSecret
                                            ? 'Leave empty to keep current secret'
                                            : 'Enter client secret'}
                                        required={!data.okta.hasClientSecret} />
                                </div>

                                <div class="flex justify-end gap-2">
                                    {#if data.okta.enabled}
                                        <Button
                                            variant="outline"
                                            type="button"
                                            class="cursor-pointer"
                                            onclick={() => (oktaShowForm = false)}>
                                            Cancel
                                        </Button>
                                    {/if}
                                    <Button
                                        type="submit"
                                        disabled={oktaIsSubmitting}
                                        class="cursor-pointer">
                                        {#if oktaIsSubmitting}
                                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                                            Saving...
                                        {:else}
                                            {data.okta.enabled ? 'Update' : 'Enable'}
                                        {/if}
                                    </Button>
                                </div>
                            </form>
                        {/if}
                    </Card.Content>
                </Card.Root>
            {/if}
        </div>
    </div>
</div>
