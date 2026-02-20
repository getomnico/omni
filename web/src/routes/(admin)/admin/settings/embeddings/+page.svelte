<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import * as Dialog from '$lib/components/ui/dialog'
    import { CheckCircle2, Loader2, Info, Pencil, Trash2, Server, Zap } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageData } from './$types'
    import {
        EMBEDDING_PROVIDER_TYPES,
        PROVIDER_LABELS,
        type EmbeddingProviderType,
    } from '$lib/types'
    import openaiIcon from '$lib/images/icons/openai.svg'
    import awsIcon from '$lib/images/icons/aws.svg'
    import jinaIcon from '$lib/images/icons/jina.svg'
    import cohereIcon from '$lib/images/icons/cohere.svg'

    let { data }: { data: PageData } = $props()

    interface ProviderFormState {
        id?: string
        providerType: EmbeddingProviderType
        model: string
        apiKey: string
        apiUrl: string
        dimensions: string
        maxModelLen: string
    }

    const emptyForm: ProviderFormState = {
        providerType: 'jina',
        model: '',
        apiKey: '',
        apiUrl: '',
        dimensions: '',
        maxModelLen: '',
    }

    let dialogOpen = $state(false)
    let editMode = $state(false)
    let formState = $state<ProviderFormState>({ ...emptyForm })
    let isSubmitting = $state(false)
    let editingHasApiKey = $state(false)

    let confirmDialogOpen = $state(false)
    let confirmTitle = $state('')
    let confirmDescription = $state('')
    let confirmFormRef = $state<HTMLFormElement | null>(null)
    let confirmActionLabel = $state('Confirm')
    let confirmDestructive = $state(true)

    function requestConfirm(
        title: string,
        description: string,
        formEl: HTMLFormElement,
        actionLabel = 'Remove',
        destructive = true,
    ) {
        confirmTitle = title
        confirmDescription = description
        confirmFormRef = formEl
        confirmActionLabel = actionLabel
        confirmDestructive = destructive
        confirmDialogOpen = true
    }

    const providerDefaults: Record<EmbeddingProviderType, { model: string; apiUrl: string }> = {
        local: { model: 'nomic-ai/nomic-embed-text-v1.5', apiUrl: 'http://embeddings:8001/v1' },
        jina: { model: 'jina-embeddings-v3', apiUrl: 'https://api.jina.ai/v1/embeddings' },
        openai: { model: 'text-embedding-3-small', apiUrl: '' },
        cohere: { model: 'embed-v4.0', apiUrl: 'https://api.cohere.com/v2/embed' },
        bedrock: { model: 'amazon.titan-embed-text-v2:0', apiUrl: '' },
    }

    const showApiKey = (p: EmbeddingProviderType) => ['jina', 'openai', 'cohere'].includes(p)
    const showApiUrl = (p: EmbeddingProviderType) => ['local', 'jina', 'cohere'].includes(p)
    const showDimensions = (p: EmbeddingProviderType) => ['openai', 'cohere'].includes(p)

    const providerMeta: Record<
        EmbeddingProviderType,
        { description: string; icon: string | null }
    > = {
        local: {
            description: 'Self-hosted embeddings via a vLLM-compatible endpoint',
            icon: null,
        },
        jina: {
            description: 'High-quality multilingual embeddings via Jina API',
            icon: jinaIcon,
        },
        openai: {
            description: 'Embedding models via the OpenAI API',
            icon: openaiIcon,
        },
        cohere: {
            description: 'Embed models via the Cohere API',
            icon: cohereIcon,
        },
        bedrock: {
            description: 'Embedding models via AWS Bedrock with IAM auth',
            icon: awsIcon,
        },
    }

    function enhanceWithToast() {
        return async ({ result, update }: { result: any; update: () => Promise<void> }) => {
            await update()
            confirmDialogOpen = false
            if (result.type === 'success') {
                toast.success(result.data?.message || 'Operation completed successfully')
            } else if (result.type === 'failure') {
                toast.error(result.data?.error || 'Something went wrong')
            }
        }
    }

    let providerByType = $derived(
        Object.fromEntries(
            EMBEDDING_PROVIDER_TYPES.map((t) => [
                t,
                data.providers.find((p) => p.providerType === t) ?? null,
            ]),
        ) as Record<EmbeddingProviderType, (typeof data.providers)[0] | null>,
    )

    function openSetupDialog(type: EmbeddingProviderType) {
        editMode = false
        editingHasApiKey = false
        const defaults = providerDefaults[type]
        formState = {
            ...emptyForm,
            providerType: type,
            model: defaults.model,
            apiUrl: defaults.apiUrl,
        }
        dialogOpen = true
    }

    function openEditDialog(provider: (typeof data.providers)[0]) {
        editMode = true
        editingHasApiKey = provider.hasApiKey
        const config = provider.config as Record<string, string>
        formState = {
            id: provider.id,
            providerType: provider.providerType as EmbeddingProviderType,
            model: config.model || '',
            apiKey: '',
            apiUrl: config.apiUrl || '',
            dimensions: config.dimensions || '',
            maxModelLen: config.maxModelLen || '',
        }
        dialogOpen = true
    }
</script>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Embedding Providers</h1>
            <p class="text-muted-foreground mt-2">
                Configure embedding providers for semantic search. Only one provider can be active
                at a time.
            </p>
        </div>

        <!-- Provider Cards -->
        <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
            {#each EMBEDDING_PROVIDER_TYPES as type}
                {@const provider = providerByType[type]}
                {@const meta = providerMeta[type]}
                <Card.Root>
                    <Card.Header class="flex flex-row items-start justify-between space-y-0 pb-2">
                        <div class="flex items-start gap-3">
                            {#if meta.icon}
                                <img src={meta.icon} alt={PROVIDER_LABELS[type]} class="h-8 w-8" />
                            {:else}
                                <Server class="text-muted-foreground h-8 w-8" />
                            {/if}
                            <div>
                                <Card.Title class="text-lg">
                                    {PROVIDER_LABELS[type]}
                                </Card.Title>
                                {#if provider}
                                    <div class="flex items-center gap-1.5 text-sm text-green-600">
                                        <CheckCircle2 class="h-3.5 w-3.5" />
                                        Connected
                                        {#if provider.isCurrent}
                                            <span
                                                class="bg-primary/10 text-primary ml-1 rounded-full px-1.5 py-0.5 text-xs">
                                                Current
                                            </span>
                                        {/if}
                                    </div>
                                {:else}
                                    <Card.Description>{meta.description}</Card.Description>
                                {/if}
                            </div>
                        </div>
                    </Card.Header>
                    <Card.Content>
                        {#if provider}
                            {@const config = provider.config as Record<string, string>}
                            <div class="mb-3 space-y-1">
                                <p class="text-muted-foreground text-xs font-medium uppercase">
                                    Configuration
                                </p>
                                <div class="text-sm">
                                    <span class="text-muted-foreground">Model:</span>
                                    {config.model || 'Not set'}
                                </div>
                            </div>

                            <div class="flex flex-wrap gap-2">
                                {#if !provider.isCurrent}
                                    <form
                                        method="POST"
                                        action="?/setCurrent"
                                        use:enhance={enhanceWithToast}>
                                        <input type="hidden" name="id" value={provider.id} />
                                        <Button
                                            type="button"
                                            variant="outline"
                                            size="sm"
                                            class="cursor-pointer gap-1"
                                            onclick={(e) => {
                                                const form = (
                                                    e.currentTarget as HTMLElement
                                                ).closest('form')!
                                                requestConfirm(
                                                    'Switch Embedding Provider',
                                                    `This will switch the active embedding provider to "${PROVIDER_LABELS[type]}" and start re-indexing all documents with the new model. This may take a while depending on the number of documents. During re-indexing, semantic search will gradually transition to the new model.`,
                                                    form as HTMLFormElement,
                                                    'Switch & Re-index',
                                                    false,
                                                )
                                            }}>
                                            <Zap class="h-3 w-3" />
                                            Set as Current
                                        </Button>
                                    </form>
                                {/if}
                                <Button
                                    variant="outline"
                                    size="sm"
                                    class="cursor-pointer gap-1"
                                    onclick={() => openEditDialog(provider)}>
                                    <Pencil class="h-3 w-3" />
                                    Edit
                                </Button>
                                <form
                                    method="POST"
                                    action="?/delete"
                                    use:enhance={enhanceWithToast}>
                                    <input type="hidden" name="id" value={provider.id} />
                                    <Button
                                        type="button"
                                        variant="outline"
                                        size="sm"
                                        class="cursor-pointer gap-1 text-red-600 hover:text-red-700"
                                        onclick={(e) => {
                                            const form = (e.currentTarget as HTMLElement).closest(
                                                'form',
                                            )!
                                            requestConfirm(
                                                'Remove Provider',
                                                `Are you sure you want to remove "${PROVIDER_LABELS[type]}"?${provider.isCurrent ? ' This is the current provider — removing it will disable semantic search until another provider is set as current.' : ''}`,
                                                form as HTMLFormElement,
                                            )
                                        }}>
                                        <Trash2 class="h-3 w-3" />
                                        Remove
                                    </Button>
                                </form>
                            </div>
                        {:else}
                            <Button
                                class="mt-1 cursor-pointer gap-2"
                                onclick={() => openSetupDialog(type)}>
                                Connect
                            </Button>
                        {/if}
                    </Card.Content>
                </Card.Root>
            {/each}
        </div>

        <!-- Provider Setup / Edit Dialog -->
        <Dialog.Root bind:open={dialogOpen}>
            <Dialog.Content class="max-h-[90vh] overflow-y-auto sm:max-w-lg">
                <Dialog.Header>
                    <Dialog.Title>
                        {editMode ? 'Edit' : 'Connect'}
                        {PROVIDER_LABELS[formState.providerType]}
                    </Dialog.Title>
                    <Dialog.Description>
                        {editMode
                            ? 'Update the embedding provider configuration'
                            : providerMeta[formState.providerType].description}
                    </Dialog.Description>
                </Dialog.Header>

                <form
                    method="POST"
                    action={editMode ? '?/edit' : '?/add'}
                    use:enhance={() => {
                        isSubmitting = true
                        return async ({ result, update }) => {
                            await update()
                            isSubmitting = false
                            dialogOpen = false
                            if (result.type === 'success') {
                                toast.success(
                                    result.data?.message || 'Operation completed successfully',
                                )
                            } else if (result.type === 'failure') {
                                toast.error(result.data?.error || 'Something went wrong')
                            }
                        }
                    }}
                    class="space-y-4">
                    {#if editMode}
                        <input type="hidden" name="id" value={formState.id} />
                    {/if}
                    <input type="hidden" name="providerType" value={formState.providerType} />

                    <!-- Model -->
                    <div class="space-y-2">
                        <Label for="model">Model *</Label>
                        <Input
                            id="model"
                            name="model"
                            bind:value={formState.model}
                            placeholder={providerDefaults[formState.providerType].model}
                            required />
                    </div>

                    <!-- API Key -->
                    {#if showApiKey(formState.providerType)}
                        <div class="space-y-2">
                            <Label for="apiKey">
                                API Key {editingHasApiKey && editMode ? '' : '*'}
                            </Label>
                            <Input
                                id="apiKey"
                                name="apiKey"
                                type="password"
                                bind:value={formState.apiKey}
                                placeholder={editingHasApiKey && editMode
                                    ? 'Leave empty to keep current key'
                                    : 'Enter API key'}
                                required={!editMode && showApiKey(formState.providerType)} />
                        </div>
                    {/if}

                    <!-- API URL -->
                    {#if showApiUrl(formState.providerType)}
                        <div class="space-y-2">
                            <Label for="apiUrl">
                                API URL {formState.providerType === 'local' ? '*' : ''}
                            </Label>
                            <Input
                                id="apiUrl"
                                name="apiUrl"
                                bind:value={formState.apiUrl}
                                placeholder={providerDefaults[formState.providerType].apiUrl}
                                required={formState.providerType === 'local'} />
                        </div>
                    {/if}

                    <!-- Dimensions -->
                    {#if showDimensions(formState.providerType)}
                        <div class="space-y-2">
                            <Label for="dimensions">Dimensions</Label>
                            <Input
                                id="dimensions"
                                name="dimensions"
                                type="number"
                                bind:value={formState.dimensions}
                                placeholder="Leave empty for model default"
                                min="1"
                                max="4096" />
                        </div>
                    {/if}

                    <!-- Max Model Length -->
                    <div class="space-y-2">
                        <Label for="maxModelLen">Max Token Length</Label>
                        <Input
                            id="maxModelLen"
                            name="maxModelLen"
                            type="number"
                            bind:value={formState.maxModelLen}
                            placeholder="Default: 8192"
                            min="1" />
                        <p class="text-muted-foreground text-sm">
                            Maximum token length for text chunks sent to the embedding model
                        </p>
                    </div>

                    <!-- Bedrock IAM notice -->
                    {#if formState.providerType === 'bedrock'}
                        <Alert.Root>
                            <Info class="h-4 w-4" />
                            <Alert.Description>
                                Ensure your application has appropriate IAM permissions to invoke
                                Bedrock embedding models
                            </Alert.Description>
                        </Alert.Root>
                    {/if}

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

        <!-- Confirm Dialog -->
        <AlertDialog.Root bind:open={confirmDialogOpen}>
            <AlertDialog.Content>
                <AlertDialog.Header>
                    <AlertDialog.Title>{confirmTitle}</AlertDialog.Title>
                    <AlertDialog.Description>{confirmDescription}</AlertDialog.Description>
                </AlertDialog.Header>
                <AlertDialog.Footer>
                    <AlertDialog.Cancel class="cursor-pointer">Cancel</AlertDialog.Cancel>
                    <AlertDialog.Action
                        class="cursor-pointer {confirmDestructive
                            ? 'bg-red-600 text-white hover:bg-red-700'
                            : ''}"
                        onclick={() => {
                            confirmFormRef?.requestSubmit()
                        }}>
                        {confirmActionLabel}
                    </AlertDialog.Action>
                </AlertDialog.Footer>
            </AlertDialog.Content>
        </AlertDialog.Root>
    </div>
</div>
