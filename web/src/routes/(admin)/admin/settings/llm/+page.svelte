<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import * as Dialog from '$lib/components/ui/dialog'
    import {
        AlertCircle,
        CheckCircle2,
        Loader2,
        Info,
        Pencil,
        Trash2,
        Star,
        Server,
        Plus,
    } from '@lucide/svelte'
    import { cn } from '$lib/utils'
    import type { PageData, ActionData } from './$types'
    import anthropicIcon from '$lib/images/icons/anthropic.svg'
    import openaiIcon from '$lib/images/icons/openai.svg'
    import awsIcon from '$lib/images/icons/aws.svg'

    let { data, form }: { data: PageData; form: ActionData } = $props()

    type ProviderType = 'vllm' | 'anthropic' | 'bedrock' | 'openai'

    interface ProviderFormState {
        id?: string
        name: string
        providerType: ProviderType
        apiKey: string
        apiUrl: string
        regionName: string
    }

    interface ModelFormState {
        providerId: string
        modelId: string
        displayName: string
        isDefault: boolean
    }

    const emptyProviderForm: ProviderFormState = {
        name: '',
        providerType: 'anthropic',
        apiKey: '',
        apiUrl: '',
        regionName: '',
    }

    const emptyModelForm: ModelFormState = {
        providerId: '',
        modelId: '',
        displayName: '',
        isDefault: false,
    }

    let dialogOpen = $state(false)
    let editMode = $state(false)
    let formState = $state<ProviderFormState>({ ...emptyProviderForm })
    let isSubmitting = $state(false)
    let editingHasApiKey = $state(false)

    let modelDialogOpen = $state(false)
    let modelFormState = $state<ModelFormState>({ ...emptyModelForm })
    let isModelSubmitting = $state(false)

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

    const showApiKey = (p: ProviderType) => p === 'anthropic' || p === 'openai'
    const showApiUrl = (p: ProviderType) => p === 'vllm'
    const showRegion = (p: ProviderType) => p === 'bedrock'

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
        vllm: {
            label: 'vLLM (Self-hosted)',
            description: 'Self-hosted models via a vLLM-compatible endpoint',
            icon: null,
        },
    }

    const providerTypes: ProviderType[] = ['anthropic', 'openai', 'bedrock', 'vllm']

    let providerByType = $derived(
        Object.fromEntries(
            providerTypes.map((t) => [t, data.providers.find((p) => p.providerType === t) ?? null]),
        ) as Record<ProviderType, (typeof data.providers)[0] | null>,
    )

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
        }
        dialogOpen = true
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

        {#if form?.success}
            <Alert.Root variant="default" class="border-green-500 bg-green-50">
                <CheckCircle2 class="h-4 w-4 text-green-600" />
                <Alert.Title class="text-green-900">Success</Alert.Title>
                <Alert.Description class="text-green-800">
                    {form.message || 'Operation completed successfully'}
                </Alert.Description>
            </Alert.Root>
        {/if}

        {#if form?.error}
            <Alert.Root variant="destructive">
                <AlertCircle class="h-4 w-4" />
                <Alert.Title>Error</Alert.Title>
                <Alert.Description>{form.error}</Alert.Description>
            </Alert.Root>
        {/if}

        <!-- Provider Cards -->
        <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
            {#each providerTypes as type}
                {@const provider = providerByType[type]}
                {@const meta = providerMeta[type]}
                <Card.Root>
                    <Card.Header class="flex flex-row items-start justify-between space-y-0 pb-2">
                        <div class="flex items-start gap-3">
                            {#if meta.icon}
                                <img src={meta.icon} alt={meta.label} class="h-8 w-8" />
                            {:else}
                                <Server class="text-muted-foreground h-8 w-8" />
                            {/if}
                            <div>
                                <Card.Title class="text-lg">
                                    {meta.label}
                                </Card.Title>
                                {#if provider}
                                    <div class="flex items-center gap-1.5 text-sm text-green-600">
                                        <CheckCircle2 class="h-3.5 w-3.5" />
                                        Connected
                                    </div>
                                {:else}
                                    <Card.Description>{meta.description}</Card.Description>
                                {/if}
                            </div>
                        </div>
                    </Card.Header>
                    <Card.Content>
                        {#if provider}
                            <!-- Models list -->
                            {#if provider.models.length > 0}
                                <div class="mb-3 space-y-1">
                                    <p class="text-muted-foreground text-xs font-medium uppercase">
                                        Models
                                    </p>
                                    {#each provider.models as model}
                                        <div
                                            class="flex items-center justify-between rounded-md px-2 py-1.5 text-sm">
                                            <div class="flex items-center gap-2">
                                                {#if model.isDefault}
                                                    <Star
                                                        class="h-3.5 w-3.5 fill-yellow-400 text-yellow-400" />
                                                {:else}
                                                    <form
                                                        method="POST"
                                                        action="?/setDefaultModel"
                                                        use:enhance>
                                                        <input
                                                            type="hidden"
                                                            name="id"
                                                            value={model.id} />
                                                        <button
                                                            type="submit"
                                                            class="cursor-pointer"
                                                            title="Set as default">
                                                            <Star
                                                                class="text-muted-foreground h-3.5 w-3.5 hover:text-yellow-400" />
                                                        </button>
                                                    </form>
                                                {/if}
                                                <span>{model.displayName}</span>
                                                {#if model.isDefault}
                                                    <span
                                                        class="bg-primary/10 text-primary rounded-full px-1.5 py-0.5 text-xs">
                                                        Default
                                                    </span>
                                                {/if}
                                            </div>
                                            <form method="POST" action="?/deleteModel" use:enhance>
                                                <input type="hidden" name="id" value={model.id} />
                                                <button
                                                    type="button"
                                                    class="cursor-pointer text-red-400 hover:text-red-600"
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
                                                </button>
                                            </form>
                                        </div>
                                    {/each}
                                </div>
                            {/if}

                            <div class="flex flex-wrap gap-2">
                                <Button
                                    variant="outline"
                                    size="sm"
                                    class="cursor-pointer gap-1"
                                    onclick={() => openAddModelDialog(provider.id)}>
                                    <Plus class="h-3 w-3" />
                                    Add Model
                                </Button>
                                <Button
                                    variant="outline"
                                    size="sm"
                                    class="cursor-pointer gap-1"
                                    onclick={() => openEditDialog(provider)}>
                                    <Pencil class="h-3 w-3" />
                                    Edit
                                </Button>
                                <form method="POST" action="?/delete" use:enhance>
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
                                                `Are you sure you want to remove "${provider.name}" and all its models? This action cannot be undone.`,
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
                        return async ({ update }) => {
                            await update()
                            isSubmitting = false
                            dialogOpen = false
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
                                API Key {editingHasApiKey && editMode ? '' : '*'}
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
                                required={!editMode && showApiKey(formState.providerType)} />
                        </div>
                    {/if}

                    {#if showApiUrl(formState.providerType)}
                        <div class="space-y-2">
                            <Label for="apiUrl">API URL *</Label>
                            <Input
                                id="apiUrl"
                                name="apiUrl"
                                bind:value={formState.apiUrl}
                                placeholder="http://vllm:8000"
                                required={formState.providerType === 'vllm'} />
                        </div>
                    {/if}

                    {#if showRegion(formState.providerType)}
                        <div class="space-y-2">
                            <Label for="regionName">AWS Region</Label>
                            <Input
                                id="regionName"
                                name="regionName"
                                bind:value={formState.regionName}
                                placeholder="us-east-1 (auto-detected if empty)" />
                        </div>
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
                        class="cursor-pointer bg-red-600 text-white hover:bg-red-700"
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
                        return async ({ update }) => {
                            await update()
                            isModelSubmitting = false
                            modelDialogOpen = false
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
                        <input
                            type="checkbox"
                            id="isDefaultModel"
                            name="isDefault"
                            value="true"
                            checked={modelFormState.isDefault}
                            onchange={(e) =>
                                (modelFormState.isDefault = (e.target as HTMLInputElement).checked)}
                            class="h-4 w-4" />
                        <Label for="isDefaultModel" class="font-normal">Set as default model</Label>
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
