<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { Badge } from '$lib/components/ui/badge'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import * as Dialog from '$lib/components/ui/dialog'
    import { Loader2, Info, Pencil, Trash2, Server } from '@lucide/svelte'
    import { cn } from '$lib/utils'
    import { toast } from 'svelte-sonner'
    import type { PageData } from './$types'
    import anthropicIcon from '$lib/images/icons/anthropic.svg'
    import openaiIcon from '$lib/images/icons/openai.svg'
    import awsIcon from '$lib/images/icons/aws.svg'
    import geminiIcon from '$lib/images/icons/gemini.svg'
    import azureIcon from '$lib/images/icons/azure.svg'
    import googleIcon from '$lib/images/icons/google.svg'

    let { data }: { data: PageData } = $props()

    type ProviderType =
        | 'openai_compatible'
        | 'anthropic'
        | 'bedrock'
        | 'openai'
        | 'gemini'
        | 'azure_foundry'
        | 'vertex_ai'

    interface ProviderFormState {
        id?: string
        name: string
        providerType: ProviderType
        apiKey: string
        apiUrl: string
        regionName: string
        projectId: string
    }

    interface ModelFormState {
        providerId: string
        modelId: string
        displayName: string
        isDefault: boolean
        isSecondary: boolean
    }

    const emptyProviderForm: ProviderFormState = {
        name: '',
        providerType: 'anthropic',
        apiKey: '',
        apiUrl: '',
        regionName: '',
        projectId: '',
    }

    const emptyModelForm: ModelFormState = {
        providerId: '',
        modelId: '',
        displayName: '',
        isDefault: false,
        isSecondary: false,
    }

    let dialogOpen = $state(false)
    let editMode = $state(false)
    let formState = $state<ProviderFormState>({ ...emptyProviderForm })
    let isSubmitting = $state(false)
    let editingHasApiKey = $state(false)

    let modelDialogOpen = $state(false)
    let modelFormState = $state<ModelFormState>({ ...emptyModelForm })
    let isModelSubmitting = $state(false)

    let manageMode = $state<Record<string, boolean>>({})
    let roleForms = $state<Record<string, HTMLFormElement>>({})

    let confirmDialogOpen = $state(false)
    let confirmTitle = $state('')
    let confirmDescription = $state('')
    let confirmFormRef = $state<HTMLFormElement | null>(null)

    function requestConfirm(title: string, description: string, formEl: HTMLFormElement) {
        confirmTitle = title
        confirmDescription = description
        confirmFormRef = formEl
        confirmDialogOpen = true
    }

    const showApiKey = (p: ProviderType) =>
        p === 'anthropic' || p === 'openai' || p === 'gemini' || p === 'openai_compatible'
    const showApiUrl = (p: ProviderType) => p === 'openai_compatible' || p === 'azure_foundry'
    const apiKeyOptional = (p: ProviderType) => p === 'openai_compatible'
    const showRegion = (p: ProviderType) => p === 'bedrock' || p === 'vertex_ai'
    const showProjectId = (p: ProviderType) => p === 'vertex_ai'

    interface ProviderMeta {
        label: string
        description: string
        icon: string | null
    }

    const providerMeta: Record<ProviderType, ProviderMeta> = {
        anthropic: {
            label: 'Anthropic Claude',
            description: 'Claude models via the Anthropic API',
            icon: anthropicIcon,
        },
        openai: {
            label: 'OpenAI',
            description: 'GPT and o-series models via the OpenAI API',
            icon: openaiIcon,
        },
        bedrock: {
            label: 'AWS Bedrock',
            description: 'Access models through AWS Bedrock with IAM auth',
            icon: awsIcon,
        },
        openai_compatible: {
            label: 'OpenAI Compatible',
            description:
                'Any OpenAI-compatible endpoint (vLLM, Ollama, LM Studio, LiteLLM, OpenRouter, etc.)',
            icon: null,
        },
        gemini: {
            label: 'Google Gemini',
            description: 'Gemini models via the Google AI API',
            icon: geminiIcon,
        },
        azure_foundry: {
            label: 'Azure AI Foundry',
            description: 'OpenAI and Claude models via Azure AI Foundry',
            icon: azureIcon,
        },
        vertex_ai: {
            label: 'Google Cloud Vertex AI',
            description: 'Claude and Gemini models via Google Cloud Vertex AI',
            icon: googleIcon,
        },
    }

    const providerTypes: ProviderType[] = [
        'anthropic',
        'openai',
        'gemini',
        'azure_foundry',
        'bedrock',
        'vertex_ai',
        'openai_compatible',
    ]

    let providerByType = $derived(
        Object.fromEntries(
            providerTypes.map((t) => [t, data.providers.find((p) => p.providerType === t) ?? null]),
        ) as Record<ProviderType, (typeof data.providers)[0] | null>,
    )

    let connectedProviders = $derived(
        providerTypes
            .filter((t) => providerByType[t] !== null)
            .map((t) => ({ type: t, provider: providerByType[t]!, meta: providerMeta[t] })),
    )

    let unconfiguredTypes = $derived(providerTypes.filter((t) => providerByType[t] === null))

    function openSetupDialog(type: ProviderType) {
        editMode = false
        editingHasApiKey = false
        formState = {
            ...emptyProviderForm,
            providerType: type,
            name: providerMeta[type].label,
        }
        dialogOpen = true
    }

    function openEditDialog(provider: (typeof data.providers)[0]) {
        editMode = true
        editingHasApiKey = provider.hasApiKey
        formState = {
            id: provider.id,
            name: provider.name,
            providerType: provider.providerType as ProviderType,
            apiKey: '',
            apiUrl: (provider.config as Record<string, string>).apiUrl || '',
            regionName: (provider.config as Record<string, string>).regionName || '',
            projectId: (provider.config as Record<string, string>).projectId || '',
        }
        dialogOpen = true
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

    function openAddModelDialog(providerId: string) {
        modelFormState = {
            ...emptyModelForm,
            providerId,
        }
        modelDialogOpen = true
    }
</script>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">LLM Providers</h1>
            <p class="text-muted-foreground mt-2">
                Connect an LLM provider to enable AI-powered features
            </p>
        </div>

        <!-- Connected Provider Cards -->
        {#if connectedProviders.length > 0}
            <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
                {#each connectedProviders as { type, provider, meta } (provider.id)}
                    <Card.Root class="group/card">
                        <Card.Header class="pb-2">
                            <div class="flex items-center gap-3">
                                {#if meta.icon}
                                    <img src={meta.icon} alt={meta.label} class="h-8 w-8" />
                                {:else}
                                    <Server class="text-muted-foreground h-8 w-8" />
                                {/if}
                                <div class="flex items-center gap-2">
                                    <span class="text-base leading-tight font-semibold">
                                        {provider.name}
                                    </span>
                                    <Badge
                                        variant="secondary"
                                        class="border-green-200 bg-green-50 text-green-700 dark:border-green-800 dark:bg-green-950 dark:text-green-400">
                                        <span
                                            class="mr-1 inline-block h-1.5 w-1.5 rounded-full bg-green-500"
                                        ></span>
                                        Connected
                                    </Badge>
                                </div>
                            </div>
                            <Card.Action>
                                <div
                                    class="flex items-center gap-1 opacity-0 transition-opacity group-hover/card:opacity-100">
                                    <Button
                                        variant="ghost"
                                        size="icon"
                                        class="h-8 w-8 cursor-pointer"
                                        title="Edit provider"
                                        onclick={() => openEditDialog(provider)}>
                                        <Pencil class="h-4 w-4" />
                                    </Button>
                                    <form
                                        method="POST"
                                        action="?/delete"
                                        use:enhance={enhanceWithToast}>
                                        <input type="hidden" name="id" value={provider.id} />
                                        <Button
                                            variant="ghost"
                                            size="icon"
                                            class="hover:text-destructive h-8 w-8 cursor-pointer"
                                            title="Remove provider"
                                            onclick={(e) => {
                                                const form = (
                                                    e.currentTarget as HTMLElement
                                                ).closest('form')!
                                                requestConfirm(
                                                    'Remove Provider',
                                                    `Are you sure you want to remove "${provider.name}" and all its models? This action cannot be undone.`,
                                                    form as HTMLFormElement,
                                                )
                                            }}>
                                            <Trash2 class="h-4 w-4" />
                                        </Button>
                                    </form>
                                </div>
                            </Card.Action>
                        </Card.Header>

                        <Card.Content class="pb-0">
                            <!-- Models section header -->
                            <div class="flex items-center justify-between px-1">
                                <span
                                    class="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
                                    Models
                                </span>
                                {#if provider.models.length > 0}
                                    <Button
                                        variant="link"
                                        size="sm"
                                        class={cn(
                                            'h-auto cursor-pointer px-1.5 py-0.5 text-xs font-medium transition-opacity',
                                            manageMode[provider.id]
                                                ? 'text-primary'
                                                : 'text-muted-foreground opacity-0 group-hover/card:opacity-100',
                                        )}
                                        onclick={() =>
                                            (manageMode[provider.id] = !manageMode[provider.id])}>
                                        {manageMode[provider.id] ? 'Done' : 'Manage'}
                                    </Button>
                                {/if}
                            </div>

                            <!-- Model list -->
                            {#if provider.models.length > 0}
                                <div class="mt-1 space-y-0.5">
                                    {#each provider.models as model (model.id)}
                                        <!-- Hidden forms for role cycling -->
                                        <form
                                            method="POST"
                                            action="?/setDefaultModel"
                                            use:enhance={enhanceWithToast}
                                            class="hidden"
                                            bind:this={roleForms[`default-${model.id}`]}>
                                            <input type="hidden" name="id" value={model.id} />
                                        </form>
                                        <form
                                            method="POST"
                                            action="?/setSecondaryModel"
                                            use:enhance={enhanceWithToast}
                                            class="hidden"
                                            bind:this={roleForms[`secondary-${model.id}`]}>
                                            <input type="hidden" name="id" value={model.id} />
                                        </form>

                                        <div
                                            class="flex min-h-8 items-center justify-between rounded-md px-1">
                                            <div class="flex items-center gap-2.5">
                                                <span
                                                    class={cn(
                                                        'block h-2.5 w-2.5 shrink-0 rounded-full',
                                                        model.isDefault
                                                            ? 'bg-amber-400'
                                                            : model.isSecondary
                                                              ? 'bg-blue-500'
                                                              : 'bg-muted-foreground/40',
                                                    )}></span>

                                                <div class="flex items-baseline gap-2">
                                                    <span class="text-sm font-medium">
                                                        {model.displayName}
                                                    </span>
                                                    {#if model.isDefault}
                                                        <span
                                                            class="text-xs font-medium text-amber-600 dark:text-amber-400">
                                                            Default
                                                        </span>
                                                    {:else if model.isSecondary}
                                                        <span
                                                            class="text-xs font-medium text-blue-600 dark:text-blue-400">
                                                            Secondary
                                                        </span>
                                                    {/if}
                                                </div>
                                            </div>

                                            {#if manageMode[provider.id]}
                                                <div class="flex items-center gap-1">
                                                    {#if !model.isDefault}
                                                        <Button
                                                            variant="outline"
                                                            size="sm"
                                                            class="h-6 cursor-pointer px-2 text-xs"
                                                            onclick={() =>
                                                                roleForms[
                                                                    `default-${model.id}`
                                                                ]?.requestSubmit()}>
                                                            Set default
                                                        </Button>
                                                    {/if}
                                                    {#if !model.isSecondary}
                                                        <Button
                                                            variant="outline"
                                                            size="sm"
                                                            class="h-6 cursor-pointer px-2 text-xs"
                                                            onclick={() =>
                                                                roleForms[
                                                                    `secondary-${model.id}`
                                                                ]?.requestSubmit()}>
                                                            Set secondary
                                                        </Button>
                                                    {/if}
                                                    <form
                                                        method="POST"
                                                        action="?/deleteModel"
                                                        use:enhance={enhanceWithToast}
                                                        class="flex items-center">
                                                        <input
                                                            type="hidden"
                                                            name="id"
                                                            value={model.id} />
                                                        <Button
                                                            variant="outline"
                                                            size="icon"
                                                            class="hover:text-destructive h-6 w-6 cursor-pointer"
                                                            title="Remove model"
                                                            onclick={(e) => {
                                                                const form = (
                                                                    e.currentTarget as HTMLElement
                                                                ).closest('form')!
                                                                requestConfirm(
                                                                    'Remove Model',
                                                                    `Are you sure you want to remove "${model.displayName}"? Existing chats using this model will fall back to the default.`,
                                                                    form as HTMLFormElement,
                                                                )
                                                            }}>
                                                            <Trash2 class="h-3.5 w-3.5" />
                                                        </Button>
                                                    </form>
                                                </div>
                                            {/if}
                                        </div>
                                    {/each}
                                </div>
                            {/if}

                            <Button
                                variant="ghost"
                                size="sm"
                                class="text-muted-foreground hover:text-foreground mt-1 cursor-pointer gap-1.5 text-sm font-medium"
                                onclick={() => openAddModelDialog(provider.id)}>
                                <span class="text-base leading-none">+</span>
                                Add model
                            </Button>
                        </Card.Content>
                    </Card.Root>
                {/each}
            </div>
        {/if}

        <!-- Connect a Provider -->
        {#if unconfiguredTypes.length > 0}
            <div class="space-y-3">
                <h2 class="text-lg font-semibold">Connect a Provider</h2>
                <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
                    {#each unconfiguredTypes as type}
                        {@const meta = providerMeta[type]}
                        <button
                            type="button"
                            class="cursor-pointer text-left"
                            onclick={() => openSetupDialog(type)}>
                            <Card.Root
                                class="hover:border-foreground/20 hover:bg-accent/50 h-full transition-colors">
                                <Card.Header>
                                    <div class="flex items-start gap-3">
                                        {#if meta.icon}
                                            <img src={meta.icon} alt={meta.label} class="h-8 w-8" />
                                        {:else}
                                            <Server class="text-muted-foreground h-8 w-8" />
                                        {/if}
                                        <div>
                                            <Card.Title class="text-sm">{meta.label}</Card.Title>
                                            <Card.Description class="text-xs">
                                                {meta.description}
                                            </Card.Description>
                                        </div>
                                    </div>
                                </Card.Header>
                            </Card.Root>
                        </button>
                    {/each}
                </div>
            </div>
        {/if}

        <!-- Provider Setup / Edit Dialog (connection fields only) -->
        <Dialog.Root bind:open={dialogOpen}>
            <Dialog.Content class="max-h-[90vh] overflow-y-auto sm:max-w-lg">
                <Dialog.Header>
                    <Dialog.Title>
                        {editMode ? 'Edit' : 'Connect'}
                        {providerMeta[formState.providerType].label}
                    </Dialog.Title>
                    <Dialog.Description>
                        {editMode
                            ? 'Update the connection configuration'
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

                    <div class="space-y-2">
                        <Label for="name">Display Name *</Label>
                        <Input
                            id="name"
                            name="name"
                            bind:value={formState.name}
                            placeholder="e.g., Production Claude"
                            required />
                    </div>

                    {#if showApiKey(formState.providerType)}
                        <div class="space-y-2">
                            <Label for="apiKey">
                                API Key {apiKeyOptional(formState.providerType)
                                    ? '(optional)'
                                    : editingHasApiKey && editMode
                                      ? ''
                                      : '*'}
                            </Label>
                            <Input
                                id="apiKey"
                                name="apiKey"
                                type="password"
                                bind:value={formState.apiKey}
                                placeholder={editingHasApiKey && editMode
                                    ? 'Leave empty to keep current key'
                                    : formState.providerType === 'anthropic'
                                      ? 'sk-ant-...'
                                      : 'sk-...'}
                                required={!editMode &&
                                    showApiKey(formState.providerType) &&
                                    !apiKeyOptional(formState.providerType)} />
                        </div>
                    {/if}

                    {#if showApiUrl(formState.providerType)}
                        <div class="space-y-2">
                            <Label for="apiUrl">
                                {formState.providerType === 'azure_foundry'
                                    ? 'Endpoint URL'
                                    : 'Base URL'} *
                            </Label>
                            <Input
                                id="apiUrl"
                                name="apiUrl"
                                bind:value={formState.apiUrl}
                                placeholder={formState.providerType === 'azure_foundry'
                                    ? 'https://<project>.services.ai.azure.com'
                                    : 'https://api.example.com'}
                                required={showApiUrl(formState.providerType)} />
                        </div>
                    {/if}

                    {#if formState.providerType === 'azure_foundry'}
                        <Alert.Root>
                            <Info class="h-4 w-4" />
                            <Alert.Description>
                                Authentication uses Azure Managed Identity. Ensure the VM or
                                container has a managed identity with the Cognitive Services User
                                role assigned.
                            </Alert.Description>
                        </Alert.Root>
                    {/if}

                    {#if showRegion(formState.providerType)}
                        <div class="space-y-2">
                            <Label for="regionName">
                                {formState.providerType === 'vertex_ai'
                                    ? 'GCP Region'
                                    : 'AWS Region'}
                                {formState.providerType === 'vertex_ai' ? ' *' : ''}
                            </Label>
                            <Input
                                id="regionName"
                                name="regionName"
                                bind:value={formState.regionName}
                                placeholder={formState.providerType === 'vertex_ai'
                                    ? 'us-central1'
                                    : 'us-east-1 (auto-detected if empty)'}
                                required={formState.providerType === 'vertex_ai'} />
                        </div>
                    {/if}

                    {#if showProjectId(formState.providerType)}
                        <div class="space-y-2">
                            <Label for="projectId">GCP Project ID *</Label>
                            <Input
                                id="projectId"
                                name="projectId"
                                bind:value={formState.projectId}
                                placeholder="my-gcp-project"
                                required />
                        </div>
                    {/if}

                    {#if formState.providerType === 'vertex_ai'}
                        <Alert.Root>
                            <Info class="h-4 w-4" />
                            <Alert.Description>
                                Authentication uses Application Default Credentials (ADC). Ensure
                                the VM or container has a service account with Vertex AI
                                permissions, or set the GOOGLE_APPLICATION_CREDENTIALS environment
                                variable.
                            </Alert.Description>
                        </Alert.Root>
                    {/if}

                    {#if formState.providerType === 'bedrock'}
                        <Alert.Root>
                            <Info class="h-4 w-4" />
                            <Alert.Description>
                                Ensure your application has appropriate IAM permissions to invoke
                                Bedrock models
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

        <!-- Confirm Delete Dialog -->
        <AlertDialog.Root bind:open={confirmDialogOpen}>
            <AlertDialog.Content>
                <AlertDialog.Header>
                    <AlertDialog.Title>{confirmTitle}</AlertDialog.Title>
                    <AlertDialog.Description>{confirmDescription}</AlertDialog.Description>
                </AlertDialog.Header>
                <AlertDialog.Footer>
                    <AlertDialog.Cancel class="cursor-pointer">Cancel</AlertDialog.Cancel>
                    <AlertDialog.Action
                        class="bg-destructive text-destructive-foreground hover:bg-destructive/90 cursor-pointer"
                        onclick={() => {
                            confirmFormRef?.requestSubmit()
                        }}>
                        Remove
                    </AlertDialog.Action>
                </AlertDialog.Footer>
            </AlertDialog.Content>
        </AlertDialog.Root>

        <!-- Add Model Dialog -->
        <Dialog.Root bind:open={modelDialogOpen}>
            <Dialog.Content class="sm:max-w-md">
                <Dialog.Header>
                    <Dialog.Title>Add Model</Dialog.Title>
                    <Dialog.Description>Add a new model to this provider</Dialog.Description>
                </Dialog.Header>

                <form
                    method="POST"
                    action="?/addModel"
                    use:enhance={() => {
                        isModelSubmitting = true
                        return async ({ result, update }) => {
                            await update()
                            isModelSubmitting = false
                            modelDialogOpen = false
                            if (result.type === 'success') {
                                toast.success(result.data?.message || 'Model added successfully')
                            } else if (result.type === 'failure') {
                                toast.error(result.data?.error || 'Something went wrong')
                            }
                        }
                    }}
                    class="space-y-4">
                    <input type="hidden" name="providerId" value={modelFormState.providerId} />

                    <div class="space-y-2">
                        <Label for="modelId">Model ID *</Label>
                        <Input
                            id="modelId"
                            name="modelId"
                            bind:value={modelFormState.modelId}
                            placeholder="e.g., claude-sonnet-4-5-20250929"
                            required />
                    </div>

                    <div class="space-y-2">
                        <Label for="displayName">Display Name *</Label>
                        <Input
                            id="displayName"
                            name="displayName"
                            bind:value={modelFormState.displayName}
                            placeholder="e.g., Claude Sonnet 4.5"
                            required />
                    </div>

                    <div class="flex items-center gap-2">
                        <Checkbox
                            id="isDefaultModel"
                            name="isDefault"
                            value="true"
                            checked={modelFormState.isDefault}
                            onCheckedChange={(v) => (modelFormState.isDefault = v === true)} />
                        <Label for="isDefaultModel" class="font-normal">Set as default model</Label>
                    </div>

                    <div class="flex items-center gap-2">
                        <Checkbox
                            id="isSecondaryModel"
                            name="isSecondary"
                            value="true"
                            checked={modelFormState.isSecondary}
                            onCheckedChange={(v) => (modelFormState.isSecondary = v === true)} />
                        <Label for="isSecondaryModel" class="font-normal">
                            Set as secondary (lightweight) model
                        </Label>
                    </div>

                    <Dialog.Footer>
                        <Button
                            variant="outline"
                            type="button"
                            class="cursor-pointer"
                            onclick={() => (modelDialogOpen = false)}>
                            Cancel
                        </Button>
                        <Button type="submit" disabled={isModelSubmitting} class="cursor-pointer">
                            {#if isModelSubmitting}
                                <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                                Adding...
                            {:else}
                                Add Model
                            {/if}
                        </Button>
                    </Dialog.Footer>
                </form>
            </Dialog.Content>
        </Dialog.Root>
    </div>
</div>
