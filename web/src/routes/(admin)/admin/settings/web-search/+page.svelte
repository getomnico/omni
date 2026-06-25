<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Textarea } from '$lib/components/ui/textarea'
    import { Badge } from '$lib/components/ui/badge'
    import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '$lib/components/ui/card'
    import * as Dialog from '$lib/components/ui/dialog'
    import { Loader2 } from '@lucide/svelte'
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
    import exaIcon from '$lib/images/icons/exa.png'
    import serperIcon from '$lib/images/icons/serper.png'
    import braveIcon from '$lib/images/icons/brave.png'
    import firecrawlIcon from '$lib/images/icons/firecrawl.png'

    let { data }: { data: PageData } = $props()

    type ProviderKind = 'search' | 'fetch'
    type ProviderType = WebSearchProviderType | WebFetchProviderType

    interface ProviderFormState {
        id?: string
        kind: ProviderKind
        providerType: ProviderType
        apiKey: string
        baseUrl: string
        hasApiKey: boolean
    }

    interface ProviderMeta {
        description: string
        icon: string
    }

    const emptyForm: ProviderFormState = {
        kind: 'search',
        providerType: 'exa',
        apiKey: '',
        baseUrl: '',
        hasApiKey: false,
    }

    const searchProviderMeta: Record<WebSearchProviderType, ProviderMeta> = {
        exa: {
            description: 'AI-optimized semantic web search with strong source discovery.',
            icon: exaIcon,
        },
        serper: {
            description: 'Fast, cost-effective Google search results through Serper.dev.',
            icon: serperIcon,
        },
        brave: {
            description: 'Independent web index with broad coverage through Brave Search API.',
            icon: braveIcon,
        },
    }

    const fetchProviderMeta: Record<WebFetchProviderType, ProviderMeta> = {
        exa: {
            description: 'Fetch page contents through Exa for teams already using Exa.',
            icon: exaIcon,
        },
        firecrawl: {
            description: 'High-quality page scraping with robust markdown extraction.',
            icon: firecrawlIcon,
        },
    }

    let dialogOpen = $state(false)
    let editMode = $state(false)
    let formState = $state<ProviderFormState>({ ...emptyForm })
    let isSubmitting = $state(false)

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

    function enhanceDialogForm() {
        isSubmitting = true
        return async ({ result, update }: { result: any; update: () => Promise<void> }) => {
            await update()
            isSubmitting = false
            if (result.type === 'success') {
                dialogOpen = false
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

    function providerLabel(kind: ProviderKind, providerType: ProviderType): string {
        return kind === 'search'
            ? WEB_SEARCH_PROVIDER_LABELS[providerType as WebSearchProviderType]
            : WEB_FETCH_PROVIDER_LABELS[providerType as WebFetchProviderType]
    }

    function providerDescription(kind: ProviderKind, providerType: ProviderType): string {
        return kind === 'search'
            ? searchProviderMeta[providerType as WebSearchProviderType].description
            : fetchProviderMeta[providerType as WebFetchProviderType].description
    }

    function dialogAction(): string {
        if (formState.kind === 'search') return editMode ? '?/editSearchProvider' : '?/addSearchProvider'
        return editMode ? '?/editFetchProvider' : '?/addFetchProvider'
    }

    function openSetupDialog(kind: ProviderKind, providerType: ProviderType) {
        editMode = false
        formState = {
            ...emptyForm,
            kind,
            providerType,
        }
        dialogOpen = true
    }

    function openEditDialog(
        kind: ProviderKind,
        provider: (typeof data.searchProviders)[0] | (typeof data.fetchProviders)[0],
    ) {
        editMode = true
        formState = {
            id: provider.id,
            kind,
            providerType: provider.providerType as ProviderType,
            apiKey: '',
            baseUrl: baseUrl(provider.config) || '',
            hasApiKey: provider.hasApiKey,
        }
        dialogOpen = true
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
            <h2 class="text-xl font-semibold">Search Providers</h2>
            <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
                {#each WEB_SEARCH_PROVIDER_TYPES as type}
                    {@const provider = searchProviderByType[type]}
                    {@const meta = searchProviderMeta[type]}
                    <Card class="group/card">
                        <CardHeader class="pb-2">
                            <div class="flex items-center gap-3">
                                <div
                                    class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                    <img
                                        src={meta.icon}
                                        alt={WEB_SEARCH_PROVIDER_LABELS[type]}
                                        class="h-7 w-7 object-contain" />
                                </div>
                                <div class="min-w-0 flex-1">
                                    <div class="flex flex-wrap items-center gap-2">
                                        <span class="text-base leading-tight font-semibold">
                                            {WEB_SEARCH_PROVIDER_LABELS[type]}
                                        </span>
                                        {#if provider}
                                            <Badge
                                                variant="secondary"
                                                class="border-green-200 bg-green-50 text-green-700 dark:border-green-800 dark:bg-green-950 dark:text-green-400">
                                                <span
                                                    class="mr-1 inline-block h-1.5 w-1.5 rounded-full bg-green-500"
                                                ></span>
                                                Connected
                                            </Badge>
                                            {#if provider.isCurrent}<Badge>Current</Badge>{/if}
                                        {/if}
                                    </div>
                                </div>
                            </div>
                        </CardHeader>
                        <CardContent class="flex-1 space-y-2">
                            <CardDescription>{meta.description}</CardDescription>
                            {#if provider && baseUrl(provider.config)}
                                <div class="text-sm">
                                    <span class="text-muted-foreground">Base URL:</span>
                                    {baseUrl(provider.config)}
                                </div>
                            {/if}
                        </CardContent>
                        <CardFooter class="flex flex-wrap gap-2">
                            {#if provider}
                                <Button
                                    size="sm"
                                    variant="outline"
                                    class="cursor-pointer"
                                    onclick={() => openEditDialog('search', provider)}>
                                    Edit
                                </Button>
                                {#if !provider.isCurrent}
                                    <form
                                        method="POST"
                                        action="?/setCurrentSearchProvider"
                                        use:enhance={enhanceWithToast}>
                                        <input type="hidden" name="id" value={provider.id} />
                                        <Button type="submit" size="sm" class="cursor-pointer">
                                            Set current
                                        </Button>
                                    </form>
                                {/if}
                                <form
                                    method="POST"
                                    action="?/deleteSearchProvider"
                                    use:enhance={enhanceWithToast}>
                                    <input type="hidden" name="id" value={provider.id} />
                                    <Button
                                        type="submit"
                                        variant="ghost"
                                        size="sm"
                                        class="hover:text-destructive cursor-pointer">
                                        Remove
                                    </Button>
                                </form>
                            {:else}
                                <Button
                                    size="sm"
                                    class="cursor-pointer"
                                    onclick={() => openSetupDialog('search', type)}>
                                    Connect
                                </Button>
                            {/if}
                        </CardFooter>
                    </Card>
                {/each}
            </div>
        </section>

        <section class="space-y-4">
            <h2 class="text-xl font-semibold">Fetch Providers</h2>
            <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
                {#each WEB_FETCH_PROVIDER_TYPES as type}
                    {@const provider = fetchProviderByType[type]}
                    {@const meta = fetchProviderMeta[type]}
                    <Card class="group/card">
                        <CardHeader class="pb-2">
                            <div class="flex items-center gap-3">
                                <div
                                    class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                    <img
                                        src={meta.icon}
                                        alt={WEB_FETCH_PROVIDER_LABELS[type]}
                                        class="h-7 w-7 object-contain" />
                                </div>
                                <div class="min-w-0 flex-1">
                                    <div class="flex flex-wrap items-center gap-2">
                                        <span class="text-base leading-tight font-semibold">
                                            {WEB_FETCH_PROVIDER_LABELS[type]}
                                        </span>
                                        {#if provider}
                                            <Badge
                                                variant="secondary"
                                                class="border-green-200 bg-green-50 text-green-700 dark:border-green-800 dark:bg-green-950 dark:text-green-400">
                                                <span
                                                    class="mr-1 inline-block h-1.5 w-1.5 rounded-full bg-green-500"
                                                ></span>
                                                Connected
                                            </Badge>
                                            {#if provider.isCurrent}<Badge>Current</Badge>{/if}
                                        {/if}
                                    </div>
                                </div>
                            </div>
                        </CardHeader>
                        <CardContent class="flex-1 space-y-2">
                            <CardDescription>{meta.description}</CardDescription>
                            {#if provider && baseUrl(provider.config)}
                                <div class="text-sm">
                                    <span class="text-muted-foreground">Base URL:</span>
                                    {baseUrl(provider.config)}
                                </div>
                            {/if}
                        </CardContent>
                        <CardFooter class="flex flex-wrap gap-2">
                            {#if provider}
                                <Button
                                    size="sm"
                                    variant="outline"
                                    class="cursor-pointer"
                                    onclick={() => openEditDialog('fetch', provider)}>
                                    Edit
                                </Button>
                                {#if !provider.isCurrent}
                                    <form
                                        method="POST"
                                        action="?/setCurrentFetchProvider"
                                        use:enhance={enhanceWithToast}>
                                        <input type="hidden" name="id" value={provider.id} />
                                        <Button type="submit" size="sm" class="cursor-pointer">
                                            Set current
                                        </Button>
                                    </form>
                                {/if}
                                <form
                                    method="POST"
                                    action="?/deleteFetchProvider"
                                    use:enhance={enhanceWithToast}>
                                    <input type="hidden" name="id" value={provider.id} />
                                    <Button
                                        type="submit"
                                        variant="ghost"
                                        size="sm"
                                        class="hover:text-destructive cursor-pointer">
                                        Remove
                                    </Button>
                                </form>
                            {:else}
                                <Button
                                    size="sm"
                                    class="cursor-pointer"
                                    onclick={() => openSetupDialog('fetch', type)}>
                                    Connect
                                </Button>
                            {/if}
                        </CardFooter>
                    </Card>
                {/each}
            </div>
        </section>

        <Dialog.Root bind:open={dialogOpen}>
            <Dialog.Content class="max-h-[90vh] overflow-y-auto sm:max-w-lg">
                <Dialog.Header>
                    <Dialog.Title>
                        {editMode ? 'Edit' : 'Connect'}
                        {providerLabel(formState.kind, formState.providerType)}
                    </Dialog.Title>
                    <Dialog.Description>
                        {editMode
                            ? `Update the ${formState.kind === 'search' ? 'search' : 'fetch'} provider configuration`
                            : providerDescription(formState.kind, formState.providerType)}
                    </Dialog.Description>
                </Dialog.Header>

                <form method="POST" action={dialogAction()} use:enhance={enhanceDialogForm} class="space-y-4">
                    {#if editMode}
                        <input type="hidden" name="id" value={formState.id} />
                    {/if}
                    <input type="hidden" name="providerType" value={formState.providerType} />

                    <div class="space-y-2">
                        <Label for="apiKey">API Key {formState.hasApiKey && editMode ? '' : '*'}</Label>
                        <Input
                            id="apiKey"
                            name="apiKey"
                            type="password"
                            bind:value={formState.apiKey}
                            placeholder={formState.hasApiKey && editMode
                                ? 'Leave empty to keep current key'
                                : 'Enter API key'}
                            required={!editMode} />
                    </div>

                    <div class="space-y-2">
                        <Label for="baseUrl">Base URL (optional)</Label>
                        <Input
                            id="baseUrl"
                            name="baseUrl"
                            bind:value={formState.baseUrl}
                            placeholder="Provider default" />
                    </div>

                    <Dialog.Footer>
                        <Button
                            variant="outline"
                            type="button"
                            class="cursor-pointer"
                            onclick={() => (dialogOpen = false)}>
                            Cancel
                        </Button>
                        <Button type="submit" disabled={isSubmitting} class="cursor-pointer">
                            {#if isSubmitting}
                                <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                                Saving...
                            {:else}
                                {editMode ? 'Update' : 'Connect'}
                            {/if}
                        </Button>
                    </Dialog.Footer>
                </form>
            </Dialog.Content>
        </Dialog.Root>

        <section class="space-y-4">
            <div>
                <h2 class="text-xl font-semibold">URL Blocklist</h2>
            </div>
            <Card>
                <CardHeader>
                    <CardTitle>Web access policy</CardTitle>
                    <CardDescription>
                        Block exact domains, wildcard domains like *.example.com, or exact http(s) URL prefixes.
                    </CardDescription>
                </CardHeader>
                <CardContent>
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
                </CardContent>
            </Card>
        </section>
    </div>
</div>
