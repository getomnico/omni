<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import { Badge } from '$lib/components/ui/badge'
    import { toast } from 'svelte-sonner'
    import { Check, Copy, KeyRound } from '@lucide/svelte'
    import { formatDate } from '$lib/utils/sources'
    import OAuthClientConfigDialog from '$lib/components/oauth-integrations/oauth-client-config-dialog.svelte'
    import type { PageProps } from './$types'

    let { data }: PageProps = $props()

    type Provider = (typeof data.providers)[number]

    let activeProvider = $state<Provider | null>(null)
    let redirectUriCopied = $state(false)
    let copyResetTimer: ReturnType<typeof setTimeout> | null = null

    function closeDialog() {
        activeProvider = null
    }

    async function copyRedirectUri() {
        await navigator.clipboard.writeText(data.redirectUri)
        redirectUriCopied = true
        toast.success('Redirect URI copied')
        if (copyResetTimer) clearTimeout(copyResetTimer)
        copyResetTimer = setTimeout(() => {
            redirectUriCopied = false
            copyResetTimer = null
        }, 2000)
    }
</script>

<svelte:head>
    <title>OAuth Integrations - Settings - Omni</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">OAuth Integrations</h1>
            <p class="text-muted-foreground mt-2">
                Configure OAuth apps used by personal integrations and integration actions.
            </p>
        </div>

        <Card.Root>
            <Card.Header>
                <Card.Title>Connector OAuth clients</Card.Title>
                <Card.Description>
                    Each provider uses one provider-level OAuth client. A provider can power
                    multiple source types, such as Google Drive and Gmail.
                </Card.Description>
            </Card.Header>
            <Card.Content class="space-y-6">
                <div class="space-y-2">
                    <div class="text-sm font-medium">Shared redirect URI</div>
                    <p class="text-muted-foreground text-sm">
                        Use this callback URL when creating OAuth apps in Google, GitHub, Microsoft,
                        or another provider.
                    </p>
                    <div class="flex gap-2">
                        <code
                            class="bg-muted text-muted-foreground flex-1 rounded-md px-3 py-2 text-sm break-all">
                            {data.redirectUri}
                        </code>
                        <Button variant="outline" class="cursor-pointer" onclick={copyRedirectUri}>
                            {#if redirectUriCopied}
                                <Check class="h-4 w-4 text-green-600" />
                                Copied
                            {:else}
                                <Copy class="h-4 w-4" />
                                Copy
                            {/if}
                        </Button>
                    </div>
                </div>
                {#if data.providers.length > 0}
                    <div class="overflow-hidden rounded-lg border">
                        <div
                            class="bg-muted/50 text-muted-foreground grid grid-cols-[1.4fr_0.8fr_1fr_0.8fr] gap-4 px-4 py-3 text-sm font-medium">
                            <div>Provider</div>
                            <div>Status</div>
                            <div>Last updated</div>
                            <div class="text-right">Action</div>
                        </div>
                        {#each data.providers as provider}
                            <div
                                class="grid grid-cols-[1.4fr_0.8fr_1fr_0.8fr] items-center gap-4 border-t px-4 py-3 text-sm">
                                <div class="flex items-center gap-2 font-medium">
                                    <KeyRound class="text-muted-foreground h-4 w-4" />
                                    {provider.displayName}
                                </div>
                                <div>
                                    {#if provider.configured}
                                        <Badge variant="secondary">Configured</Badge>
                                    {:else}
                                        <Badge variant="outline">Not configured</Badge>
                                    {/if}
                                </div>
                                <div class="text-muted-foreground">
                                    {formatDate(provider.updatedAt)}
                                </div>
                                <div class="text-right">
                                    <Button
                                        size="sm"
                                        variant={provider.configured ? 'outline' : 'default'}
                                        class="cursor-pointer"
                                        onclick={() => (activeProvider = provider)}>
                                        {provider.configured ? 'Edit' : 'Configure'}
                                    </Button>
                                </div>
                            </div>
                        {/each}
                    </div>
                {:else}
                    <div class="py-12 text-center">
                        <p class="text-muted-foreground text-sm">
                            No OAuth-capable connector manifests are currently registered.
                        </p>
                    </div>
                {/if}
            </Card.Content>
        </Card.Root>
    </div>
</div>

{#if activeProvider}
    <OAuthClientConfigDialog
        open={activeProvider !== null}
        provider={activeProvider.provider}
        displayName={activeProvider.displayName}
        configured={activeProvider.configured}
        config={activeProvider.config}
        onSaved={closeDialog}
        onCancel={closeDialog} />
{/if}
