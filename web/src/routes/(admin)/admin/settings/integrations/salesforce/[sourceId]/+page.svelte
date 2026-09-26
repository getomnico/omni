<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Loader2, KeyRound, AlertTriangle } from '@lucide/svelte'
    import { Badge } from '$lib/components/ui/badge'
    import { onMount, untrack } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import { toast } from 'svelte-sonner'
    import type { PageProps } from './$types'
    import salesforceLogo from '$lib/images/icons/salesforce.svg'
    import OAuthClientConfigDialog from '$lib/components/oauth-integrations/oauth-client-config-dialog.svelte'

    let { data }: PageProps = $props()

    const isNoSync = $derived(
        data.source.sourceType === 'salesforce' &&
            typeof data.source.config === 'object' &&
            data.source.config !== null &&
            !Array.isArray(data.source.config) &&
            (data.source.config as Record<string, unknown>).sync_enabled === false,
    )

    let enabled = $state(untrack(() => data.source.isActive))
    let originalEnabled = untrack(() => data.source.isActive)

    let isSubmitting = $state(false)
    let hasUnsavedChanges = $derived(enabled !== originalEnabled)
    let skipUnsavedCheck = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    onMount(() => {
        beforeUnloadHandler = (e: BeforeUnloadEvent) => {
            if (hasUnsavedChanges && !skipUnsavedCheck) {
                e.preventDefault()
                e.returnValue = ''
            }
        }

        window.addEventListener('beforeunload', beforeUnloadHandler)
        void checkUserOAuthStatus()

        return () => {
            if (beforeUnloadHandler) {
                window.removeEventListener('beforeunload', beforeUnloadHandler)
            }
        }
    })

    beforeNavigate(({ cancel }) => {
        if (hasUnsavedChanges && !skipUnsavedCheck) {
            const shouldLeave = confirm(
                'You have unsaved changes. Are you sure you want to leave this page?',
            )
            if (!shouldLeave) {
                cancel()
            }
        }
    })

    type ScopedOAuthProvider = {
        provider: string
        displayName: string
        configured: boolean
        updatedAt: string | null
        config: Record<string, unknown>
    }

    let oauthDialogOpen = $state(false)
    let oauthDialogProvider = $state<ScopedOAuthProvider | null>(null)
    let oauthDialogLoading = $state(false)
    // null = unknown/failed check; false = no usable per-user OAuth client
    let userOAuthConfigured = $state<boolean | null>(null)

    async function loadOAuthConfig(): Promise<ScopedOAuthProvider> {
        const response = await fetch('/api/connector-configs')
        if (!response.ok) {
            throw new Error('Failed to load Salesforce OAuth configuration')
        }
        const configs = (await response.json()) as Array<{
            provider: string
            config: Record<string, unknown>
            updatedAt: string
        }>
        const provider = `salesforce:${data.source.id}`
        const saved = configs.find((item) => item.provider === provider)
        const config = saved?.config ?? {}
        const configured =
            typeof config.oauth_client_id === 'string' &&
            config.oauth_client_id.length > 0 &&
            ((typeof config.oauth_client_secret === 'string' &&
                config.oauth_client_secret.length > 0) ||
                config.oauth_dynamic_client_registration === 'true')
        return {
            provider,
            displayName: `Salesforce — ${data.source.name} user OAuth`,
            configured,
            updatedAt: saved?.updatedAt ?? null,
            config,
        }
    }

    async function checkUserOAuthStatus() {
        oauthDialogLoading = true
        try {
            userOAuthConfigured = (await loadOAuthConfig()).configured
        } catch {
            userOAuthConfigured = null
        } finally {
            oauthDialogLoading = false
        }
    }

    async function openOAuthConfig() {
        oauthDialogLoading = true
        try {
            oauthDialogProvider = await loadOAuthConfig()
            userOAuthConfigured = oauthDialogProvider.configured
            oauthDialogOpen = true
        } catch (err) {
            toast.error(err instanceof Error ? err.message : 'Failed to load Salesforce OAuth')
        } finally {
            oauthDialogLoading = false
        }
    }

    function closeOAuthConfig() {
        oauthDialogOpen = false
        oauthDialogProvider = null
    }
</script>

<svelte:head>
    <title>Configure Salesforce - {data.source.name}</title>
</svelte:head>
<form
    method="POST"
    use:enhance={() => {
        isSubmitting = true
        return async ({ result, update }) => {
            if (result.type === 'redirect') {
                skipUnsavedCheck = true
                hasUnsavedChanges = false

                if (beforeUnloadHandler) {
                    window.removeEventListener('beforeunload', beforeUnloadHandler)
                    beforeUnloadHandler = null
                }
            }

            await update()
            isSubmitting = false
        }
    }}>
    <Card.Root class="relative">
        <Card.Header>
            <div class="flex items-start justify-between">
                <div>
                    <Card.Title class="flex items-center gap-2">
                        <img src={salesforceLogo} alt="Salesforce" class="h-5 w-5" />
                        {data.source.name}
                    </Card.Title>
                    <Card.Description class="mt-1">
                        {#if isNoSync}
                            Enable Salesforce native read actions for this organization. No Salesforce data
                            is synced or indexed.
                        {:else}
                            Index accounts, contacts, opportunities, leads, cases, and tasks from
                            Salesforce
                        {/if}
                    </Card.Description>
                </div>
                <div class="flex items-center gap-2">
                    <Label for="enabled" class="text-sm">
                        {isNoSync ? 'Actions enabled' : 'Enabled'}
                    </Label>
                    <Switch
                        id="enabled"
                        bind:checked={enabled}
                        name="enabled"
                        class="cursor-pointer" />
                </div>
            </div>
        </Card.Header>

        <Card.Content>
            {#if isNoSync}
                <p class="text-muted-foreground text-sm">
                    Salesforce records are accessed live through agent actions using each user's
                    Salesforce authorization. There is no sync schedule, manual sync, sync status,
                    or indexed record store for this source.
                </p>
            {:else}
                <p class="text-muted-foreground text-sm">
                    All accessible CRM records will be indexed, including accounts, contacts,
                    opportunities, leads, cases, and tasks.
                </p>
            {/if}
        </Card.Content>
        <Card.Footer class="flex justify-end">
            <Button
                type="submit"
                disabled={isSubmitting || !hasUnsavedChanges}
                class="cursor-pointer">
                {#if isSubmitting}
                    <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                {/if}
                Save Configuration
            </Button>
        </Card.Footer>
    </Card.Root>
</form>

<Card.Root class="relative">
    <Card.Header>
        <div class="flex items-start justify-between">
            <div>
                <Card.Title class="flex items-center gap-2">
                    <KeyRound class="text-muted-foreground h-5 w-5" />
                    User OAuth
                </Card.Title>
                <Card.Description class="mt-1">
                    External Client App credentials used when individual Omni users authorize
                    Salesforce native actions. Stored for this source only; never shared with other
                    Salesforce orgs.
                </Card.Description>
            </div>
        </div>
    </Card.Header>
    <Card.Content>
        <p class="text-muted-foreground text-sm">
            Configure a pre-created External Client App (client ID + secret) or authenticated
            Dynamic Client Registration with an administrator-issued initial access token. The
            registration token is removed after successful registration.
        </p>
        <div class="mt-3 flex flex-wrap items-center gap-2 text-sm">
            {#if data.actionAuth.authorized}
                <Badge variant="secondary">
                    Authorized{data.actionAuth.principalEmail
                        ? ` as ${data.actionAuth.principalEmail}`
                        : ''}
                </Badge>
                {#if data.actionAuth.grantedScopes.length > 0}
                    <span class="text-muted-foreground">
                        Scopes: {data.actionAuth.grantedScopes.join(', ')}
                    </span>
                {/if}
            {:else}
                <Badge variant="outline">Not authorized for your account</Badge>
            {/if}
        </div>
        {#if isNoSync && !data.actionAuth.authorized}
            <p class="text-muted-foreground mt-3 text-sm">
                Authorize this admin account once after setup to verify and bind the Salesforce
                organization. Other users authorize from chat when they first use an action.
            </p>
        {/if}
        {#if userOAuthConfigured === false}
            <Alert.Root
                class="mt-3 border-amber-200 bg-amber-50 text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100">
                <AlertTriangle class="h-4 w-4 text-amber-500 dark:text-amber-400" />
                <Alert.Title>Salesforce actions are not configured yet</Alert.Title>
                <Alert.Description>
                    Salesforce native actions won't appear in chat until an External Client App is
                    configured for per-user OAuth. Use the button below to set one up.
                </Alert.Description>
            </Alert.Root>
        {/if}
    </Card.Content>
    <Card.Footer class="flex justify-end gap-2">
        <Button
            type="button"
            variant="outline"
            disabled={oauthDialogLoading}
            class="cursor-pointer"
            onclick={openOAuthConfig}>
            {#if oauthDialogLoading}
                <Loader2 class="mr-2 h-4 w-4 animate-spin" />
            {/if}
            {oauthDialogProvider?.configured
                ? 'Edit user OAuth client'
                : 'Configure user OAuth client'}
        </Button>
        <Button
            type="button"
            variant="outline"
            class="cursor-pointer"
            disabled={oauthDialogLoading || userOAuthConfigured !== true}
            href={`/api/oauth/start?source_id=${data.source.id}&flow=user_read&return_to=${encodeURIComponent(`/admin/settings/integrations/salesforce/${data.source.id}`)}`}>
            {#if userOAuthConfigured === false}
                Configure OAuth client first
            {:else if data.actionAuth.authorized}
                Re-authorize Salesforce
            {:else}
                Authorize Salesforce
            {/if}
        </Button>
    </Card.Footer>
</Card.Root>

{#if oauthDialogOpen && oauthDialogProvider}
    <OAuthClientConfigDialog
        open={oauthDialogOpen}
        provider={oauthDialogProvider.provider}
        displayName={oauthDialogProvider.displayName}
        configured={oauthDialogProvider.configured}
        registrationRequiresInitialAccessToken={data.oauth.registrationRequiresInitialAccessToken}
        config={oauthDialogProvider.config}
        onSaved={() => {
            oauthDialogOpen = false
            oauthDialogProvider = null
            void checkUserOAuthStatus()
        }}
        onCancel={closeOAuthConfig} />
{/if}
