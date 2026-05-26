<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Badge } from '$lib/components/ui/badge'
    import { Info, KeyRound } from '@lucide/svelte'
    import { formatDate } from '$lib/utils/sources'
    import OAuthClientConfigDialog from '$lib/components/oauth-integrations/oauth-client-config-dialog.svelte'
    import type { PageProps } from './$types'

    let { data }: PageProps = $props()

    type Provider = (typeof data.providers)[number]

    let activeProvider = $state<Provider | null>(null)

    function closeDialog() {
        activeProvider = null
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

        <Alert.Root>
            <Info class="h-4 w-4" />
            <Alert.Title>Not for sign-in</Alert.Title>
            <Alert.Description>
                These OAuth apps let users connect external accounts and authorize integration
                actions. They do not control how users sign in to Omni; use Authentication for login
                providers.
            </Alert.Description>
        </Alert.Root>

        <Card.Root>
            <Card.Header>
                <Card.Title>Connector OAuth clients</Card.Title>
                <Card.Description>
                    Each provider uses one shared redirect URI and one provider-level OAuth client.
                    A provider can power multiple source types, such as Google Drive and Gmail.
                </Card.Description>
            </Card.Header>
            <Card.Content>
                {#if data.providers.length > 0}
                    <div class="overflow-hidden rounded-lg border">
                        <div
                            class="bg-muted/50 text-muted-foreground grid grid-cols-[1.1fr_1.4fr_0.8fr_1.4fr_1fr_0.8fr] gap-4 px-4 py-3 text-sm font-medium">
                            <div>Provider</div>
                            <div>Used by</div>
                            <div>Status</div>
                            <div>Redirect URI</div>
                            <div>Last updated</div>
                            <div class="text-right">Action</div>
                        </div>
                        {#each data.providers as provider}
                            <div
                                class="grid grid-cols-[1.1fr_1.4fr_0.8fr_1.4fr_1fr_0.8fr] items-center gap-4 border-t px-4 py-3 text-sm">
                                <div class="flex items-center gap-2 font-medium">
                                    <KeyRound class="text-muted-foreground h-4 w-4" />
                                    {provider.displayName}
                                </div>
                                <div class="text-muted-foreground">
                                    {provider.sourceTypeNames.join(', ')}
                                </div>
                                <div>
                                    {#if provider.configured}
                                        <Badge variant="secondary">Configured</Badge>
                                    {:else}
                                        <Badge variant="outline">Not configured</Badge>
                                    {/if}
                                </div>
                                <code class="text-muted-foreground truncate text-xs">
                                    {data.redirectUri}
                                </code>
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

                    <div class="mt-4 space-y-1">
                        <div class="text-sm font-medium">Shared redirect URI</div>
                        <code
                            class="bg-muted text-muted-foreground block rounded-md px-3 py-2 text-sm break-all">
                            {data.redirectUri}
                        </code>
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
        redirectUri={data.redirectUri}
        configured={activeProvider.configured}
        config={activeProvider.config}
        onSaved={closeDialog}
        onCancel={closeDialog} />
{/if}
