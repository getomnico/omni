<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Textarea } from '$lib/components/ui/textarea'
    import { Badge } from '$lib/components/ui/badge'
    import * as Card from '$lib/components/ui/card'
    import { Globe, Search, FileText, Trash2, CheckCircle2 } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageData } from './$types'
    import {
        WEB_SEARCH_PROVIDER_TYPES,
        WEB_SEARCH_PROVIDER_LABELS,
        WEB_FETCH_PROVIDER_TYPES,
        WEB_FETCH_PROVIDER_LABELS,
        type WebSearchProviderType,
        type WebFetchProviderType,
    } from '$lib/types'

    let { data }: { data: PageData } = $props()

    function enhanceWithToast() {
        return async ({ result, update }: { result: any; update: () => Promise<void> }) => {
            await update()
            if (result.type === 'success') {
                toast.success(result.data?.message || 'Operation completed successfully')
            } else if (result.type === 'failure') {
                toast.error(result.data?.error || 'Something went wrong')
            }
        }
    }

    let searchProviderByType = $derived(
        Object.fromEntries(
            WEB_SEARCH_PROVIDER_TYPES.map((type) => [
                type,
                data.searchProviders.find((provider) => provider.providerType === type) ?? null,
            ]),
        ) as Record<WebSearchProviderType, (typeof data.searchProviders)[0] | null>,
    )

    let fetchProviderByType = $derived(
        Object.fromEntries(
            WEB_FETCH_PROVIDER_TYPES.map((type) => [
                type,
                data.fetchProviders.find((provider) => provider.providerType === type) ?? null,
            ]),
        ) as Record<WebFetchProviderType, (typeof data.fetchProviders)[0] | null>,
    )

    function baseUrl(config: Record<string, unknown>): string | null {
        return typeof config.baseUrl === 'string' && config.baseUrl ? config.baseUrl : null
    }
</script>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Web Providers</h1>
            <p class="text-muted-foreground mt-2">
                Configure public web search, page fetch providers, and URL access controls for agent tools.
            </p>
        </div>

        <section class="space-y-4">
            <div class="flex items-center gap-2">
                <Search class="h-5 w-5" />
                <h2 class="text-xl font-semibold">Search Providers</h2>
            </div>
            <div class="grid grid-cols-1 gap-4 lg:grid-cols-3">
                {#each WEB_SEARCH_PROVIDER_TYPES as type}
                    {@const provider = searchProviderByType[type]}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>{WEB_SEARCH_PROVIDER_LABELS[type]}</Card.Title>
                            <Card.Description>Public web search API integration.</Card.Description>
                        </Card.Header>
                        <Card.Content class="space-y-4">
                            {#if provider}
                                <div class="flex flex-wrap items-center gap-2">
                                    <Badge variant="secondary">Configured</Badge>
                                    {#if provider.isCurrent}<Badge>Current</Badge>{/if}
                                    {#if provider.hasApiKey}<Badge variant="outline">API key saved</Badge>{/if}
                                </div>
                                {#if baseUrl(provider.config)}
                                    <p class="text-muted-foreground text-sm">Base URL: {baseUrl(provider.config)}</p>
                                {/if}
                                <div class="flex gap-2">
                                    {#if !provider.isCurrent}
                                        <form method="POST" action="?/setCurrentSearchProvider" use:enhance={enhanceWithToast}>
                                            <input type="hidden" name="id" value={provider.id} />
                                            <Button type="submit" size="sm" class="cursor-pointer">
                                                <CheckCircle2 class="h-4 w-4" /> Set current
                                            </Button>
                                        </form>
                                    {/if}
                                    <form method="POST" action="?/deleteSearchProvider" use:enhance={enhanceWithToast}>
                                        <input type="hidden" name="id" value={provider.id} />
                                        <Button type="submit" variant="outline" size="sm" class="cursor-pointer">
                                            <Trash2 class="h-4 w-4" /> Remove
                                        </Button>
                                    </form>
                                </div>
                            {:else}
                                <form method="POST" action="?/addSearchProvider" use:enhance={enhanceWithToast} class="space-y-3">
                                    <input type="hidden" name="providerType" value={type} />
                                    <div class="space-y-1.5">
                                        <Label for={`search-${type}-api-key`}>API key</Label>
                                        <Input id={`search-${type}-api-key`} name="apiKey" type="password" />
                                    </div>
                                    <div class="space-y-1.5">
                                        <Label for={`search-${type}-base-url`}>Base URL (optional)</Label>
                                        <Input id={`search-${type}-base-url`} name="baseUrl" placeholder="Provider default" />
                                    </div>
                                    <Button type="submit" class="w-full cursor-pointer">Connect</Button>
                                </form>
                            {/if}
                        </Card.Content>
                    </Card.Root>
                {/each}
            </div>
        </section>

        <section class="space-y-4">
            <div class="flex items-center gap-2">
                <FileText class="h-5 w-5" />
                <h2 class="text-xl font-semibold">Fetch Providers</h2>
            </div>
            <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
                {#each WEB_FETCH_PROVIDER_TYPES as type}
                    {@const provider = fetchProviderByType[type]}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>{WEB_FETCH_PROVIDER_LABELS[type]}</Card.Title>
                            <Card.Description>Fetch readable content for fetch_web_page.</Card.Description>
                        </Card.Header>
                        <Card.Content class="space-y-4">
                            {#if provider}
                                <div class="flex flex-wrap items-center gap-2">
                                    <Badge variant="secondary">Configured</Badge>
                                    {#if provider.isCurrent}<Badge>Current</Badge>{/if}
                                    {#if provider.hasApiKey}<Badge variant="outline">API key saved</Badge>{/if}
                                </div>
                                {#if baseUrl(provider.config)}
                                    <p class="text-muted-foreground text-sm">Base URL: {baseUrl(provider.config)}</p>
                                {/if}
                                <div class="flex gap-2">
                                    {#if !provider.isCurrent}
                                        <form method="POST" action="?/setCurrentFetchProvider" use:enhance={enhanceWithToast}>
                                            <input type="hidden" name="id" value={provider.id} />
                                            <Button type="submit" size="sm" class="cursor-pointer">
                                                <CheckCircle2 class="h-4 w-4" /> Set current
                                            </Button>
                                        </form>
                                    {/if}
                                    <form method="POST" action="?/deleteFetchProvider" use:enhance={enhanceWithToast}>
                                        <input type="hidden" name="id" value={provider.id} />
                                        <Button type="submit" variant="outline" size="sm" class="cursor-pointer">
                                            <Trash2 class="h-4 w-4" /> Remove
                                        </Button>
                                    </form>
                                </div>
                            {:else}
                                <form method="POST" action="?/addFetchProvider" use:enhance={enhanceWithToast} class="space-y-3">
                                    <input type="hidden" name="providerType" value={type} />
                                    <div class="space-y-1.5">
                                        <Label for={`fetch-${type}-api-key`}>API key</Label>
                                        <Input id={`fetch-${type}-api-key`} name="apiKey" type="password" />
                                    </div>
                                    <div class="space-y-1.5">
                                        <Label for={`fetch-${type}-base-url`}>Base URL (optional)</Label>
                                        <Input id={`fetch-${type}-base-url`} name="baseUrl" placeholder="Provider default" />
                                    </div>
                                    <Button type="submit" class="w-full cursor-pointer">Connect</Button>
                                </form>
                            {/if}
                        </Card.Content>
                    </Card.Root>
                {/each}
            </div>
        </section>

        <section class="space-y-4">
            <div class="flex items-center gap-2">
                <Globe class="h-5 w-5" />
                <h2 class="text-xl font-semibold">URL Blocklist</h2>
            </div>
            <Card.Root>
                <Card.Header>
                    <Card.Title>Web access policy</Card.Title>
                    <Card.Description>
                        Block exact domains, wildcard domains like *.example.com, or exact http(s) URL prefixes.
                    </Card.Description>
                </Card.Header>
                <Card.Content>
                    <form method="POST" action="?/savePolicy" use:enhance={enhanceWithToast} class="space-y-4">
                        <div class="space-y-1.5">
                            <Label for="blocklist">Blocklist patterns</Label>
                            <Textarea
                                id="blocklist"
                                name="blocklist"
                                rows={8}
                                value={data.blocklist.join('\n')}
                                placeholder={'example.com\n*.example.com\nhttps://example.com/private'} />
                        </div>
                        <Button type="submit" class="cursor-pointer">Save policy</Button>
                    </form>
                </Card.Content>
            </Card.Root>
        </section>
    </div>
</div>
