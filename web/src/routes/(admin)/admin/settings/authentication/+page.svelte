<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Switch } from '$lib/components/ui/switch'
    import { Loader2, Info, Pencil, KeyRound } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageData } from './$types'
    import googleIcon from '$lib/images/icons/google.svg'
    import oktaIcon from '$lib/images/icons/okta.svg'
    import microsoftIcon from '$lib/images/icons/microsoft.svg'

    let { data }: { data: PageData } = $props()

    let enabled = $state(data.google.enabled)
    let clientId = $state(data.google.clientId)
    let clientSecret = $state('')
    let isSubmitting = $state(false)
    let showForm = $state(false)

    let oktaEnabled = $state(data.okta?.enabled ?? false)
    let oktaDomain = $state(data.okta?.oktaDomain ?? '')
    let oktaClientId = $state(data.okta?.clientId ?? '')
    let oktaClientSecret = $state('')
    let oktaIsSubmitting = $state(false)
    let oktaShowForm = $state(false)

    let entraEnabled = $state(data.entra?.enabled ?? false)
    let entraTenant = $state(data.entra?.tenant ?? '')
    let entraClientId = $state(data.entra?.clientId ?? '')
    let entraClientSecret = $state('')
    let entraIsSubmitting = $state(false)
    let entraShowForm = $state(false)

    let passwordEnabled = $state(data.passwordAuthEnabled)
    let passwordIsSubmitting = $state(false)

    let oktaDisableFormRef = $state<HTMLFormElement | null>(null)
    let entraDisableFormRef = $state<HTMLFormElement | null>(null)
    let googleDisableFormRef = $state<HTMLFormElement | null>(null)
    let passwordFormRef = $state<HTMLFormElement | null>(null)

    function handleOktaSwitch() {
        if (oktaEnabled) {
            oktaDisableFormRef?.requestSubmit()
        } else {
            oktaShowForm = true
        }
    }

    function handleEntraSwitch() {
        if (entraEnabled) {
            entraDisableFormRef?.requestSubmit()
        } else {
            entraShowForm = true
        }
    }

    function handleGoogleSwitch() {
        if (enabled) {
            googleDisableFormRef?.requestSubmit()
        } else {
            showForm = true
        }
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

        <div class="space-y-4">
            <!-- Okta -->
            {#if data.oktaSsoAvailable}
                <Card.Root>
                    <Card.Header
                        class={oktaShowForm || (!data.okta?.enabled && !oktaEnabled) ? 'pb-2' : ''}>
                        <div class="flex items-center gap-3">
                            <div
                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                <img src={oktaIcon} alt="Okta" class="h-7 w-7 object-contain" />
                            </div>
                            <div>
                                <div class="text-base leading-tight font-semibold">Okta</div>
                                <p class="text-muted-foreground mt-0.5 text-sm">
                                    Sign in with Okta SSO
                                </p>
                            </div>
                        </div>
                        <Card.Action>
                            <div class="flex items-center gap-2">
                                {#if oktaEnabled && !oktaShowForm}
                                    <Button
                                        variant="ghost"
                                        size="icon"
                                        class="h-8 w-8 cursor-pointer"
                                        title="Edit"
                                        onclick={() => (oktaShowForm = true)}>
                                        <Pencil class="h-4 w-4" />
                                    </Button>
                                {/if}
                                <form
                                    method="POST"
                                    action="?/updateOkta"
                                    bind:this={oktaDisableFormRef}
                                    class="hidden"
                                    use:enhance={() => {
                                        oktaIsSubmitting = true
                                        return async ({ result, update }) => {
                                            oktaIsSubmitting = false
                                            await update()
                                            oktaEnabled = data.okta?.enabled
                                            if (result.type === 'success') {
                                                toast.success(
                                                    result.data?.message || 'Okta SSO disabled',
                                                )
                                                oktaShowForm = false
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
                                </form>
                                <Switch
                                    checked={oktaEnabled}
                                    disabled={oktaIsSubmitting}
                                    onCheckedChange={handleOktaSwitch}
                                    class="cursor-pointer" />
                            </div>
                        </Card.Action>
                    </Card.Header>
                    {#if oktaEnabled && !oktaShowForm}
                        <Card.Content>
                            <div class="min-w-0 space-y-1 text-sm">
                                <div class="flex gap-1">
                                    <span class="text-muted-foreground shrink-0">Domain:</span>
                                    <span class="truncate font-mono">{data.okta?.oktaDomain}</span>
                                </div>
                                <div class="flex gap-1">
                                    <span class="text-muted-foreground shrink-0">Client ID:</span>
                                    <span class="truncate font-mono">{data.okta?.clientId}</span>
                                </div>
                            </div>
                        </Card.Content>
                    {:else if oktaShowForm}
                        <Card.Content>
                            <form
                                method="POST"
                                action="?/updateOkta"
                                use:enhance={() => {
                                    oktaIsSubmitting = true
                                    return async ({ result, update }) => {
                                        oktaIsSubmitting = false
                                        await update()
                                        oktaEnabled = data.okta?.enabled
                                        if (result.type === 'success') {
                                            toast.success(
                                                result.data?.message || 'Okta SSO enabled',
                                            )
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
                                        Client Secret {data.okta?.hasClientSecret ? '' : '*'}
                                    </Label>
                                    <Input
                                        id="oktaClientSecret"
                                        name="clientSecret"
                                        type="password"
                                        bind:value={oktaClientSecret}
                                        placeholder={data.okta?.hasClientSecret
                                            ? 'Leave empty to keep current secret'
                                            : 'Enter client secret'}
                                        required={!data.okta?.hasClientSecret} />
                                </div>

                                <div class="flex justify-end gap-2">
                                    <Button
                                        variant="outline"
                                        type="button"
                                        class="cursor-pointer"
                                        onclick={() => {
                                            oktaShowForm = false
                                            if (!data.okta?.enabled) oktaEnabled = false
                                        }}>
                                        Cancel
                                    </Button>
                                    <Button
                                        type="submit"
                                        disabled={oktaIsSubmitting}
                                        class="cursor-pointer">
                                        {#if oktaIsSubmitting}
                                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                                            Saving...
                                        {:else}
                                            Save
                                        {/if}
                                    </Button>
                                </div>
                            </form>
                        </Card.Content>
                    {/if}
                </Card.Root>
            {/if}

            <!-- Microsoft Entra ID -->
            {#if data.entraSsoAvailable}
                <Card.Root>
                    <Card.Header
                        class={entraShowForm || (!data.entra?.enabled && !entraEnabled)
                            ? 'pb-2'
                            : ''}>
                        <div class="flex items-center gap-3">
                            <div
                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                <img
                                    src={microsoftIcon}
                                    alt="Microsoft"
                                    class="h-7 w-7 object-contain" />
                            </div>
                            <div>
                                <div class="text-base leading-tight font-semibold">
                                    Microsoft Entra ID
                                </div>
                                <p class="text-muted-foreground mt-0.5 text-sm">
                                    Sign in with Microsoft Entra ID
                                </p>
                            </div>
                        </div>
                        <Card.Action>
                            <div class="flex items-center gap-2">
                                {#if entraEnabled && !entraShowForm}
                                    <Button
                                        variant="ghost"
                                        size="icon"
                                        class="h-8 w-8 cursor-pointer"
                                        title="Edit"
                                        onclick={() => (entraShowForm = true)}>
                                        <Pencil class="h-4 w-4" />
                                    </Button>
                                {/if}
                                <form
                                    method="POST"
                                    action="?/updateEntra"
                                    bind:this={entraDisableFormRef}
                                    class="hidden"
                                    use:enhance={() => {
                                        entraIsSubmitting = true
                                        return async ({ result, update }) => {
                                            entraIsSubmitting = false
                                            await update()
                                            entraEnabled = data.entra?.enabled
                                            if (result.type === 'success') {
                                                toast.success(
                                                    result.data?.message || 'Entra SSO disabled',
                                                )
                                                entraShowForm = false
                                            } else if (result.type === 'failure') {
                                                toast.error(
                                                    result.data?.error || 'Something went wrong',
                                                )
                                            }
                                        }
                                    }}>
                                    <input type="hidden" name="enabled" value="false" />
                                    <input type="hidden" name="tenant" value="" />
                                    <input type="hidden" name="clientId" value="" />
                                    <input type="hidden" name="clientSecret" value="" />
                                </form>
                                <Switch
                                    checked={entraEnabled}
                                    disabled={entraIsSubmitting}
                                    onCheckedChange={handleEntraSwitch}
                                    class="cursor-pointer" />
                            </div>
                        </Card.Action>
                    </Card.Header>
                    {#if entraEnabled && !entraShowForm}
                        <Card.Content>
                            <div class="min-w-0 space-y-1 text-sm">
                                <div class="flex gap-1">
                                    <span class="text-muted-foreground shrink-0">Tenant:</span>
                                    <span class="truncate font-mono">{data.entra?.tenant}</span>
                                </div>
                                <div class="flex gap-1">
                                    <span class="text-muted-foreground shrink-0">Client ID:</span>
                                    <span class="truncate font-mono">{data.entra?.clientId}</span>
                                </div>
                            </div>
                        </Card.Content>
                    {:else if entraShowForm}
                        <Card.Content>
                            <form
                                method="POST"
                                action="?/updateEntra"
                                use:enhance={() => {
                                    entraIsSubmitting = true
                                    return async ({ result, update }) => {
                                        entraIsSubmitting = false
                                        await update()
                                        entraEnabled = data.entra?.enabled
                                        if (result.type === 'success') {
                                            toast.success(
                                                result.data?.message || 'Entra SSO enabled',
                                            )
                                            entraClientSecret = ''
                                            entraShowForm = false
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
                                        Register an application in Microsoft Entra ID, create a
                                        client secret, and paste the credentials here. Set the
                                        redirect URI to
                                        <code class="bg-muted rounded px-1 text-sm"
                                            >{'{app_url}'}/auth/entra/callback</code>
                                    </Alert.Description>
                                </Alert.Root>

                                <div class="space-y-2">
                                    <Label for="entraTenant">Tenant ID *</Label>
                                    <Input
                                        id="entraTenant"
                                        name="tenant"
                                        bind:value={entraTenant}
                                        placeholder="contoso.onmicrosoft.com"
                                        required />
                                </div>

                                <div class="space-y-2">
                                    <Label for="entraClientId">Client ID *</Label>
                                    <Input
                                        id="entraClientId"
                                        name="clientId"
                                        bind:value={entraClientId}
                                        placeholder="00000000-0000-0000-0000-000000000000"
                                        required />
                                </div>

                                <div class="space-y-2">
                                    <Label for="entraClientSecret">
                                        Client Secret {data.entra?.hasClientSecret ? '' : '*'}
                                    </Label>
                                    <Input
                                        id="entraClientSecret"
                                        name="clientSecret"
                                        type="password"
                                        bind:value={entraClientSecret}
                                        placeholder={data.entra?.hasClientSecret
                                            ? 'Leave empty to keep current secret'
                                            : 'Enter client secret'}
                                        required={!data.entra?.hasClientSecret} />
                                </div>

                                <div class="flex justify-end gap-2">
                                    <Button
                                        variant="outline"
                                        type="button"
                                        class="cursor-pointer"
                                        onclick={() => {
                                            entraShowForm = false
                                            if (!data.entra?.enabled) entraEnabled = false
                                        }}>
                                        Cancel
                                    </Button>
                                    <Button
                                        type="submit"
                                        disabled={entraIsSubmitting}
                                        class="cursor-pointer">
                                        {#if entraIsSubmitting}
                                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                                            Saving...
                                        {:else}
                                            Save
                                        {/if}
                                    </Button>
                                </div>
                            </form>
                        </Card.Content>
                    {/if}
                </Card.Root>
            {/if}

            <!-- Google -->
            <Card.Root>
                <Card.Header class={showForm || (!data.google.enabled && !enabled) ? 'pb-2' : ''}>
                    <div class="flex items-center gap-3">
                        <div
                            class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                            <img
                                src={googleIcon}
                                alt="Google"
                                class="h-7 w-7 object-contain" />
                        </div>
                        <div>
                            <div class="text-base leading-tight font-semibold">Google</div>
                            <p class="text-muted-foreground mt-0.5 text-sm">
                                Sign in with Google Workspace
                            </p>
                        </div>
                    </div>
                    <Card.Action>
                        <div class="flex items-center gap-2">
                            {#if enabled && !showForm}
                                <Button
                                    variant="ghost"
                                    size="icon"
                                    class="h-8 w-8 cursor-pointer"
                                    title="Edit"
                                    onclick={() => (showForm = true)}>
                                    <Pencil class="h-4 w-4" />
                                </Button>
                            {/if}
                            <form
                                method="POST"
                                action="?/update"
                                bind:this={googleDisableFormRef}
                                class="hidden"
                                use:enhance={() => {
                                    isSubmitting = true
                                    return async ({ result, update }) => {
                                        isSubmitting = false
                                        await update()
                                        enabled = data.google.enabled
                                        if (result.type === 'success') {
                                            toast.success(
                                                result.data?.message || 'Google Auth disabled',
                                            )
                                            showForm = false
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
                            </form>
                            <Switch
                                checked={enabled}
                                disabled={isSubmitting}
                                onCheckedChange={handleGoogleSwitch}
                                class="cursor-pointer" />
                        </div>
                    </Card.Action>
                </Card.Header>
                {#if enabled && !showForm}
                    <Card.Content>
                        <div class="min-w-0 space-y-1 text-sm">
                            <div class="flex gap-1">
                                <span class="text-muted-foreground shrink-0">Client ID:</span>
                                <span class="truncate font-mono">{data.google.clientId}</span>
                            </div>
                        </div>
                    </Card.Content>
                {:else if showForm}
                    <Card.Content>
                        <form
                            method="POST"
                            action="?/update"
                            use:enhance={() => {
                                isSubmitting = true
                                return async ({ result, update }) => {
                                    isSubmitting = false
                                    await update()
                                    enabled = data.google.enabled
                                    if (result.type === 'success') {
                                        toast.success(result.data?.message || 'Google Auth enabled')
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
                                <Button
                                    variant="outline"
                                    type="button"
                                    class="cursor-pointer"
                                    onclick={() => {
                                        showForm = false
                                        if (!data.google.enabled) enabled = false
                                    }}>
                                    Cancel
                                </Button>
                                <Button
                                    type="submit"
                                    disabled={isSubmitting}
                                    class="cursor-pointer">
                                    {#if isSubmitting}
                                        <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                                        Saving...
                                    {:else}
                                        Save
                                    {/if}
                                </Button>
                            </div>
                        </form>
                    </Card.Content>
                {/if}
            </Card.Root>

            <!-- Password -->
            <Card.Root>
                <Card.Header>
                    <div class="flex items-center gap-3">
                        <div
                            class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                            <KeyRound class="h-6 w-6 text-slate-700" />
                        </div>
                        <div>
                            <div class="text-base leading-tight font-semibold">Password</div>
                            <p class="text-muted-foreground mt-0.5 text-sm">
                                Allow users to sign in with email and password
                            </p>
                        </div>
                    </div>
                    <Card.Action>
                        <form
                            method="POST"
                            action="?/updatePassword"
                            bind:this={passwordFormRef}
                            class="flex items-center"
                            use:enhance={() => {
                                passwordIsSubmitting = true
                                return async ({ result, update }) => {
                                    passwordIsSubmitting = false
                                    await update()
                                    passwordEnabled = data.passwordAuthEnabled
                                    if (result.type === 'success') {
                                        toast.success(
                                            result.data?.message ||
                                                (passwordEnabled
                                                    ? 'Password auth enabled'
                                                    : 'Password auth disabled'),
                                        )
                                    } else if (result.type === 'failure') {
                                        toast.error(result.data?.error || 'Something went wrong')
                                    }
                                }
                            }}>
                            <input type="hidden" name="enabled" value={!passwordEnabled} />
                            <Switch
                                checked={passwordEnabled}
                                disabled={passwordIsSubmitting}
                                onCheckedChange={() => passwordFormRef?.requestSubmit()}
                                class="cursor-pointer" />
                        </form>
                    </Card.Action>
                </Card.Header>
            </Card.Root>
        </div>
    </div>
</div>
