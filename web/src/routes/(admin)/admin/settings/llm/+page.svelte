<script lang="ts">
    import { deserialize, enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Alert from '$lib/components/ui/alert'
    import * as Select from '$lib/components/ui/select'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import * as Command from '$lib/components/ui/command'
    import * as Popover from '$lib/components/ui/popover'
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import * as Dialog from '$lib/components/ui/dialog'
    import * as DropdownMenu from '$lib/components/ui/dropdown-menu'
    import * as Tooltip from '$lib/components/ui/tooltip'
    import {
        Check,
        CircleAlert,
        CircleCheck,
        ChevronsUpDown,
        Info,
        Loader2,
        MoreHorizontal,
        Pencil,
        Plus,
        Server,
        Trash2,
        X,
    } from '@lucide/svelte'
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
    type ModelRole = 'default' | 'secondary' | 'unassigned'

    interface ProviderFormState {
        id?: string
        name: string
        providerType: ProviderType
        apiKey: string
        apiUrl: string
        regionName: string
        projectId: string
        visionMode: string
    }

    interface ModelFormState {
        providerId: string
        modelId: string
        displayName: string
        role: ModelRole
    }

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
            description: 'Use Ollama, vLLM, or another OpenAI-compatible API.',
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
    const multiInstanceTypes: ProviderType[] = ['openai_compatible']
    const visionModeOptions = [
        { value: 'auto', label: 'Auto-detect' },
        { value: 'on', label: 'Always allow image input' },
        { value: 'off', label: 'Never allow image input' },
    ]
    const roleOptions: { value: ModelRole; label: string; hint: string }[] = [
        { value: 'default', label: 'Default', hint: 'Chat and agent responses' },
        { value: 'secondary', label: 'Secondary', hint: 'Quick, lightweight tasks' },
        {
            value: 'unassigned',
            label: 'Unassigned',
            hint: 'Available when choosing a model in chat',
        },
    ]

    const emptyProviderForm: ProviderFormState = {
        name: '',
        providerType: 'anthropic',
        apiKey: '',
        apiUrl: '',
        regionName: '',
        projectId: '',
        visionMode: 'auto',
    }
    const emptyModelForm: ModelFormState = {
        providerId: '',
        modelId: '',
        displayName: '',
        role: 'unassigned',
    }

    let providerPickerOpen = $state(false)
    let dialogOpen = $state(false)
    let editMode = $state(false)
    let formState = $state<ProviderFormState>({ ...emptyProviderForm })
    let isSubmitting = $state(false)
    let editingHasApiKey = $state(false)
    let testResult = $state<
        { ok: true; message: string } | { ok: false; message: string; detail: string | null } | null
    >(null)

    let modelDialogOpen = $state(false)
    let modelFormState = $state<ModelFormState>({ ...emptyModelForm })
    let isModelSubmitting = $state(false)
    let modelPickerOpen = $state(false)
    let modelSearch = $state('')
    let availableModels = $state<{ modelId: string; displayName: string }[]>([])
    let modelsLoading = $state(false)

    let selectedModel = $state<(typeof data.providers)[number]['models'][number] | null>(null)
    let selectedProvider = $state<(typeof data.providers)[number] | null>(null)
    let selectedModelRole = $state<ModelRole>('unassigned')
    let modelDetailOpen = $state(false)
    let isRoleSubmitting = $state(false)

    let confirmDialogOpen = $state(false)
    let confirmTitle = $state('')
    let confirmDescription = $state('')
    let confirmFormRef = $state<HTMLFormElement | null>(null)
    let providerDeleteForms = $state<Record<string, HTMLFormElement>>({})

    let connectedProviders = $derived(
        data.providers.map((provider) => ({
            provider,
            meta: providerMeta[provider.providerType as ProviderType],
        })),
    )
    let configuredSingletonTypes = $derived(
        data.providers
            .map((provider) => provider.providerType as ProviderType)
            .filter((type) => !multiInstanceTypes.includes(type)),
    )
    let unconfiguredTypes = $derived(
        providerTypes.filter((type) => !configuredSingletonTypes.includes(type)),
    )
    let filteredModels = $derived.by(() => {
        const tokens = modelSearch.trim().toLowerCase().split(/\s+/).filter(Boolean)
        if (tokens.length === 0) return availableModels
        return availableModels.filter((model) => {
            const haystack = `${model.displayName} ${model.modelId}`.toLowerCase()
            return tokens.every((token) => haystack.includes(token))
        })
    })

    function requestConfirm(title: string, description: string, form: HTMLFormElement) {
        confirmTitle = title
        confirmDescription = description
        confirmFormRef = form
        confirmDialogOpen = true
    }

    function actionMessage(
        resultData: unknown,
        field: 'message' | 'error',
        fallback: string,
    ): string {
        if (resultData && typeof resultData === 'object') {
            const value = (resultData as Record<string, unknown>)[field]
            if (typeof value === 'string') return value
        }
        return fallback
    }

    function providerRole(model: (typeof data.providers)[number]['models'][number]): ModelRole {
        if (model.isDefault) return 'default'
        if (model.isSecondary) return 'secondary'
        return 'unassigned'
    }

    function roleLabel(role: ModelRole): string {
        return roleOptions.find((option) => option.value === role)?.label ?? 'Unassigned'
    }

    function openProviderPicker() {
        providerPickerOpen = true
    }

    function openSetupDialog(type: ProviderType) {
        providerPickerOpen = false
        editMode = false
        editingHasApiKey = false
        testResult = null
        formState = { ...emptyProviderForm, providerType: type, name: providerMeta[type].label }
        dialogOpen = true
    }

    function openEditDialog(provider: (typeof data.providers)[number]) {
        providerPickerOpen = false
        editMode = true
        editingHasApiKey = provider.hasApiKey
        testResult = null
        const config = provider.config as Record<string, unknown>
        formState = {
            id: provider.id,
            name: provider.name,
            providerType: provider.providerType as ProviderType,
            apiKey: '',
            apiUrl: typeof config.apiUrl === 'string' ? config.apiUrl : '',
            regionName: typeof config.regionName === 'string' ? config.regionName : '',
            projectId: typeof config.projectId === 'string' ? config.projectId : '',
            visionMode: typeof config.visionMode === 'string' ? config.visionMode : 'auto',
        }
        dialogOpen = true
    }

    function openModelDetail(
        provider: (typeof data.providers)[number],
        model: (typeof data.providers)[number]['models'][number],
    ) {
        selectedProvider = provider
        selectedModel = model
        selectedModelRole = providerRole(model)
        modelDetailOpen = true
    }

    function openAddModelDialog(providerId: string) {
        modelFormState = { ...emptyModelForm, providerId }
        modelDialogOpen = true
        void loadAvailableModels(providerId)
    }

    async function loadAvailableModels(providerId: string) {
        modelsLoading = true
        availableModels = []
        modelSearch = ''
        try {
            const formData = new FormData()
            formData.set('providerId', providerId)
            const response = await fetch('?/discoverModels', {
                method: 'POST',
                body: formData,
                headers: { 'x-sveltekit-action': 'true' },
            })
            const result = deserialize<
                { models?: { modelId: string; displayName: string }[] },
                { error?: string }
            >(await response.text())
            availableModels = result.type === 'success' ? (result.data?.models ?? []) : []
        } catch {
            availableModels = []
        } finally {
            modelsLoading = false
        }
    }

    function applyModelChoice(model: { modelId: string; displayName: string }) {
        modelFormState.modelId = model.modelId
        if (!modelFormState.displayName.trim()) modelFormState.displayName = model.displayName
        modelPickerOpen = false
    }

    function setConnectionError(resultData: unknown, fallback: string) {
        const result = resultData as
            | {
                  error?: string
                  provider?: string | null
                  statusCode?: number | null
                  model?: string | null
              }
            | undefined
        const details = [
            result?.provider,
            result?.model,
            result?.statusCode ? `HTTP ${result.statusCode}` : null,
        ].filter((part): part is string => typeof part === 'string')
        testResult = {
            ok: false,
            message: result?.error ?? fallback,
            detail: details.length > 0 ? details.join(' · ') : null,
        }
    }

    function resetTestResult() {
        testResult = null
    }

    const showApiKey = (type: ProviderType) =>
        type === 'anthropic' ||
        type === 'openai' ||
        type === 'gemini' ||
        type === 'openai_compatible'
    const showApiUrl = (type: ProviderType) =>
        type === 'openai_compatible' || type === 'azure_foundry'
    const apiKeyOptional = (type: ProviderType) => type === 'openai_compatible'
    const showRegion = (type: ProviderType) => type === 'bedrock' || type === 'vertex_ai'
    const showProjectId = (type: ProviderType) => type === 'vertex_ai'
</script>

<div class="bg-background min-h-full p-5 pb-24 sm:p-8 sm:pb-24">
    <div class="mx-auto max-w-[1160px] space-y-8">
        <header class="flex flex-col gap-5 sm:flex-row sm:items-start sm:justify-between">
            <div>
                <h1 class="text-3xl font-semibold tracking-tight">LLM Providers</h1>
                <p class="text-muted-foreground mt-2 text-base">
                    Manage the providers and models Omni can use.
                </p>
            </div>
            <Button onclick={openProviderPicker} class="cursor-pointer gap-2 sm:mt-1">
                <Plus class="h-4 w-4" />
                Connect provider
            </Button>
        </header>

        {#if connectedProviders.length > 0}
            <div class="grid grid-cols-1 items-start gap-5 lg:grid-cols-2">
                {#each connectedProviders as { provider, meta } (provider.id)}
                    <section
                        class="bg-card overflow-hidden rounded-2xl border shadow-none"
                        aria-labelledby={`provider-${provider.id}`}>
                        <header class="flex items-center gap-3 px-4 py-3">
                            {#if meta.icon}
                                <div
                                    class="bg-background flex h-8 w-8 shrink-0 items-center justify-center rounded-md border">
                                    <img src={meta.icon} alt="" class="h-5 w-5 object-contain" />
                                </div>
                            {:else}
                                <div
                                    class="bg-background flex h-8 w-8 shrink-0 items-center justify-center rounded-md border">
                                    <Server class="text-muted-foreground h-4 w-4" />
                                </div>
                            {/if}
                            <div class="min-w-0">
                                <h2
                                    id={`provider-${provider.id}`}
                                    class="truncate text-base font-semibold">
                                    {provider.name}
                                </h2>
                                <p class="text-muted-foreground mt-0.5 text-sm">
                                    {provider.models.length}
                                    {provider.models.length === 1 ? 'model' : 'models'}
                                </p>
                            </div>
                            <div class="ml-auto flex shrink-0 items-center gap-2">
                                <span
                                    class="text-muted-foreground hidden items-center gap-1.5 text-xs sm:inline-flex">
                                    <span class="h-1.5 w-1.5 rounded-full bg-green-600"></span>
                                    Connected
                                </span>
                                <DropdownMenu.Root>
                                    <DropdownMenu.Trigger>
                                        {#snippet child({ props })}
                                            <Button
                                                {...props}
                                                variant="ghost"
                                                size="icon"
                                                class="text-muted-foreground hover:text-foreground h-9 w-9 cursor-pointer"
                                                aria-label={`Actions for ${provider.name}`}>
                                                <MoreHorizontal class="h-5 w-5" />
                                            </Button>
                                        {/snippet}
                                    </DropdownMenu.Trigger>
                                    <DropdownMenu.Content align="end" class="w-44">
                                        <DropdownMenu.Item
                                            class="cursor-pointer"
                                            onclick={() => openEditDialog(provider)}>
                                            <Pencil class="mr-2 h-4 w-4" />
                                            Edit connection
                                        </DropdownMenu.Item>
                                        <DropdownMenu.Item
                                            class="text-destructive hover:text-destructive focus:text-destructive cursor-pointer"
                                            onclick={() =>
                                                requestConfirm(
                                                    'Remove provider',
                                                    `Are you sure you want to remove "${provider.name}" and all its models? This action cannot be undone.`,
                                                    providerDeleteForms[provider.id],
                                                )}>
                                            <Trash2 class="mr-2 h-4 w-4" />
                                            Remove provider
                                        </DropdownMenu.Item>
                                    </DropdownMenu.Content>
                                </DropdownMenu.Root>
                            </div>
                        </header>
                        <div class="border-t">
                            {#if provider.models.length > 0}
                                <div class="flex flex-col gap-1 p-2">
                                    {#each provider.models as model (model.id)}
                                        {@const role = providerRole(model)}
                                        <button
                                            type="button"
                                            class="group hover:bg-sidebar-accent hover:text-sidebar-accent-foreground dark:hover:bg-sidebar-accent/50 focus-visible:bg-sidebar-accent focus-visible:text-sidebar-accent-foreground focus-visible:ring-sidebar-ring flex min-h-11 w-full cursor-pointer items-center gap-3 rounded-md px-4 text-left transition-colors focus-visible:ring-2"
                                            onclick={() => openModelDetail(provider, model)}>
                                            <span
                                                class="min-w-0 flex-1 truncate text-sm font-medium"
                                                >{model.displayName}</span>
                                            {#if role !== 'unassigned'}
                                                <span
                                                    class={`inline-flex shrink-0 items-center gap-1.5 text-xs ${role === 'default' ? 'text-amber-700 dark:text-amber-400' : 'text-blue-700 dark:text-blue-400'}`}>
                                                    <span
                                                        class={`h-1.5 w-1.5 rounded-full ${role === 'default' ? 'bg-amber-600' : 'bg-blue-600'}`}
                                                    ></span>
                                                    {roleLabel(role)}
                                                </span>
                                            {/if}
                                            <span
                                                class="text-muted-foreground/60 shrink-0 text-lg leading-none opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
                                                aria-hidden="true">›</span>
                                        </button>
                                    {/each}
                                </div>
                            {:else}
                                <p class="text-muted-foreground px-5 py-6 text-sm">
                                    No models configured yet.
                                </p>
                            {/if}
                            <div class="p-2">
                                <Button
                                    variant="ghost"
                                    size="sm"
                                    class="cursor-pointer gap-2"
                                    onclick={() => openAddModelDialog(provider.id)}>
                                    <Plus class="h-4 w-4" />
                                    Add model
                                </Button>
                            </div>
                        </div>
                    </section>
                    <form
                        method="POST"
                        action="?/delete"
                        use:enhance={() =>
                            async ({ result, update }) => {
                                await update()
                                confirmDialogOpen = false
                                if (result.type === 'success')
                                    toast.success(
                                        actionMessage(result.data, 'message', 'Provider deleted'),
                                    )
                                else if (result.type === 'failure')
                                    toast.error(
                                        actionMessage(
                                            result.data,
                                            'error',
                                            'Could not remove provider',
                                        ),
                                    )
                            }}
                        class="hidden"
                        bind:this={providerDeleteForms[provider.id]}>
                        <input type="hidden" name="id" value={provider.id} />
                    </form>
                {/each}
            </div>
        {:else}
            <div
                class="border-muted-foreground/20 bg-card rounded-2xl border border-dashed px-6 py-12 text-center">
                <h2 class="text-lg font-semibold">No providers connected</h2>
                <p class="text-muted-foreground mt-2 text-sm">
                    Connect a provider to start using AI features.
                </p>
                <Button onclick={openProviderPicker} class="mt-5 cursor-pointer gap-2">
                    <Plus class="h-4 w-4" /> Connect provider
                </Button>
            </div>
        {/if}
    </div>
</div>

<!-- Provider picker -->
<Dialog.Root bind:open={providerPickerOpen}>
    <Dialog.Content class="sm:max-w-xl">
        <Dialog.Header>
            <Dialog.Title>Connect a provider</Dialog.Title>
            <Dialog.Description>Choose a provider to configure.</Dialog.Description>
        </Dialog.Header>
        <div class="grid auto-rows-fr gap-3 sm:grid-cols-2">
            {#each unconfiguredTypes as type (type)}
                {@const meta = providerMeta[type]}
                <button
                    type="button"
                    class="bg-card hover:bg-muted/60 flex h-full cursor-pointer items-start gap-3 rounded-xl border p-4 text-left transition-colors"
                    onclick={() => openSetupDialog(type)}>
                    {#if meta.icon}
                        <img src={meta.icon} alt="" class="h-8 w-8 object-contain" />
                    {:else}
                        <Server class="text-muted-foreground mt-1 h-7 w-7" />
                    {/if}
                    <span class="min-w-0">
                        <span class="block text-sm font-semibold">{meta.label}</span>
                        <span class="text-muted-foreground mt-1 block text-xs leading-5"
                            >{meta.description}</span>
                    </span>
                </button>
            {:else}
                <p class="text-muted-foreground col-span-full py-4 text-sm">
                    All built-in providers are already connected.
                </p>
            {/each}
        </div>
    </Dialog.Content>
</Dialog.Root>

<!-- Provider connection form. The existing connection fields and validation remain unchanged. -->
<Dialog.Root bind:open={dialogOpen}>
    <Dialog.Content class="max-h-[90vh] overflow-y-auto sm:max-w-lg">
        <Dialog.Header>
            <Dialog.Title
                >{editMode ? 'Edit' : 'Connect'}
                {providerMeta[formState.providerType].label}</Dialog.Title>
            <Dialog.Description>
                {editMode
                    ? 'Update the connection configuration'
                    : providerMeta[formState.providerType].description}
            </Dialog.Description>
        </Dialog.Header>
        <form
            id="llm-provider-form"
            method="POST"
            action={editMode ? '?/edit' : '?/add'}
            use:enhance={() => {
                isSubmitting = true
                testResult = null
                return async ({ result, update }) => {
                    isSubmitting = false
                    if (result.type === 'success') {
                        await update()
                        dialogOpen = false
                        toast.success(
                            actionMessage(
                                result.data,
                                'message',
                                'Operation completed successfully',
                            ),
                        )
                    } else if (result.type === 'failure')
                        setConnectionError(result.data, 'Connection failed')
                    else setConnectionError(null, 'Connection failed')
                }
            }}
            class="space-y-4">
            {#if editMode}<input type="hidden" name="id" value={formState.id} />{/if}
            <input type="hidden" name="providerType" value={formState.providerType} />
            <div class="space-y-2">
                <Label for="provider-name">Display Name *</Label><Input
                    id="provider-name"
                    name="name"
                    bind:value={formState.name}
                    placeholder="e.g., Production Claude"
                    required />
            </div>
            {#if showApiKey(formState.providerType)}
                <div class="space-y-2">
                    <Label for="provider-api-key"
                        >API Key {apiKeyOptional(formState.providerType)
                            ? '(optional)'
                            : editingHasApiKey && editMode
                              ? ''
                              : '*'}</Label>
                    <Input
                        id="provider-api-key"
                        name="apiKey"
                        type="password"
                        bind:value={formState.apiKey}
                        oninput={resetTestResult}
                        placeholder={editingHasApiKey && editMode
                            ? 'Leave empty to keep current key'
                            : formState.providerType === 'anthropic'
                              ? 'sk-ant-...'
                              : 'sk-...'}
                        required={!editMode && !apiKeyOptional(formState.providerType)} />
                </div>
            {/if}
            {#if showApiUrl(formState.providerType)}
                <div class="space-y-2">
                    <Label for="provider-api-url"
                        >{formState.providerType === 'azure_foundry' ? 'Endpoint URL' : 'Base URL'} *</Label>
                    <Input
                        id="provider-api-url"
                        name="apiUrl"
                        bind:value={formState.apiUrl}
                        oninput={resetTestResult}
                        placeholder={formState.providerType === 'azure_foundry'
                            ? 'https://<project>.services.ai.azure.com'
                            : 'https://openrouter.ai/api/v1'}
                        required />
                    {#if formState.providerType === 'openai_compatible'}<p
                            class="text-muted-foreground text-xs">
                            Running the bundled local-inference stack? Use <code
                                class="bg-muted rounded px-1 py-0.5 font-mono text-xs"
                                >http://llama-cpp:8000</code
                            >.
                        </p>{/if}
                </div>
            {/if}
            {#if formState.providerType === 'azure_foundry'}
                <Alert.Root
                    ><Info class="h-4 w-4" /><Alert.Description
                        >Authentication uses Azure Managed Identity. Ensure the VM or container has
                        a managed identity with the Cognitive Services User role assigned.</Alert.Description
                    ></Alert.Root>
            {/if}
            {#if formState.providerType === 'openai_compatible' || formState.providerType === 'azure_foundry'}
                <div class="space-y-2">
                    <Label>Image input</Label>
                    <input type="hidden" name="visionMode" value={formState.visionMode} />
                    <Select.Root type="single" bind:value={formState.visionMode}>
                        <Select.Trigger class="w-full cursor-pointer"
                            >{visionModeOptions.find(
                                (option) => option.value === formState.visionMode,
                            )?.label ?? 'Auto-detect'}</Select.Trigger>
                        <Select.Content
                            >{#each visionModeOptions as option (option.value)}<Select.Item
                                    value={option.value}
                                    class="cursor-pointer">{option.label}</Select.Item
                                >{/each}</Select.Content>
                    </Select.Root>
                    <p class="text-muted-foreground text-xs">
                        This setting applies to every model on this connection.
                    </p>
                </div>
            {/if}
            {#if showRegion(formState.providerType)}
                <div class="space-y-2">
                    <Label for="provider-region"
                        >{formState.providerType === 'vertex_ai'
                            ? 'GCP Region'
                            : 'AWS Region'}{formState.providerType === 'vertex_ai'
                            ? ' *'
                            : ''}</Label
                    ><Input
                        id="provider-region"
                        name="regionName"
                        bind:value={formState.regionName}
                        oninput={resetTestResult}
                        placeholder={formState.providerType === 'vertex_ai'
                            ? 'us-central1'
                            : 'us-east-1 (auto-detected if empty)'}
                        required={formState.providerType === 'vertex_ai'} />
                </div>
            {/if}
            {#if showProjectId(formState.providerType)}
                <div class="space-y-2">
                    <Label for="provider-project">GCP Project ID *</Label><Input
                        id="provider-project"
                        name="projectId"
                        bind:value={formState.projectId}
                        oninput={resetTestResult}
                        placeholder="my-gcp-project"
                        required />
                </div>
            {/if}
            {#if formState.providerType === 'vertex_ai'}<Alert.Root
                    ><Info class="h-4 w-4" /><Alert.Description
                        >Authentication uses Application Default Credentials (ADC). Ensure the
                        service account has Vertex AI permissions.</Alert.Description
                    ></Alert.Root
                >{/if}
            {#if formState.providerType === 'bedrock'}<Alert.Root
                    ><Info class="h-4 w-4" /><Alert.Description
                        >Ensure your application has appropriate IAM permissions to invoke Bedrock
                        models.</Alert.Description
                    ></Alert.Root
                >{/if}
            {#if testResult}
                {@const connectionResult = testResult}
                {#if connectionResult.ok}<Alert.Root
                        class="border-green-500/50 bg-green-50 text-green-900 dark:bg-green-950/40 dark:text-green-100"
                        ><CircleCheck class="h-4 w-4" /><Alert.Title
                            >{connectionResult.message}</Alert.Title
                        ></Alert.Root>
                {:else}<Alert.Root variant="destructive"
                        ><CircleAlert class="h-4 w-4" /><Tooltip.Provider delayDuration={300}
                            ><Tooltip.Root
                                ><Tooltip.Trigger
                                    >{#snippet child({ props })}<Alert.Title
                                            {...props}
                                            class="cursor-help"
                                            >{connectionResult.message}</Alert.Title
                                        >{/snippet}</Tooltip.Trigger
                                ><Tooltip.Content class="max-w-sm break-words"
                                    >{connectionResult.message}</Tooltip.Content
                                ></Tooltip.Root
                            ></Tooltip.Provider
                        >{#if connectionResult.detail}<Alert.Description
                                >{connectionResult.detail}</Alert.Description
                            >{/if}</Alert.Root
                    >{/if}
            {/if}
        </form>
        <Dialog.Footer>
            <Button
                variant="outline"
                type="button"
                class="cursor-pointer"
                onclick={() => (dialogOpen = false)}>Cancel</Button>
            <Button
                type="submit"
                form="llm-provider-form"
                disabled={isSubmitting}
                class="cursor-pointer"
                >{#if isSubmitting}<Loader2 class="mr-2 h-4 w-4 animate-spin" />{editMode
                        ? 'Updating...'
                        : 'Connecting...'}{:else}{editMode ? 'Update' : 'Connect'}{/if}</Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>

<!-- Model detail dialog -->
<Dialog.Root bind:open={modelDetailOpen}>
    <Dialog.Content class="gap-0 overflow-hidden p-0 sm:max-w-xl">
        {#if selectedModel && selectedProvider}
            {@const meta = providerMeta[selectedProvider.providerType as ProviderType]}
            <div class="border-b px-6 py-4 pr-14">
                <div class="flex items-center gap-3">
                    <div
                        class="bg-background flex h-8 w-8 shrink-0 items-center justify-center rounded-md border">
                        {#if meta?.icon}
                            <img src={meta.icon} alt="" class="h-5 w-5 object-contain" />
                        {:else}
                            <Server class="text-muted-foreground h-4 w-4" />
                        {/if}
                    </div>
                    <div class="min-w-0">
                        <Dialog.Title>{selectedModel.displayName}</Dialog.Title>
                        <Dialog.Description class="mt-1 font-mono text-sm"
                            >{selectedModel.modelId}<span class="mx-2 font-sans">·</span><span
                                class="font-sans">{selectedProvider.name}</span
                            ></Dialog.Description>
                    </div>
                </div>
            </div>
            <form
                id="model-role-form"
                method="POST"
                action="?/setModelRole"
                use:enhance={() => {
                    isRoleSubmitting = true
                    return async ({ result, update }) => {
                        isRoleSubmitting = false
                        if (result.type === 'success') {
                            await update()
                            modelDetailOpen = false
                            toast.success(
                                actionMessage(result.data, 'message', 'Model role updated'),
                            )
                        } else if (result.type === 'failure')
                            toast.error(
                                actionMessage(result.data, 'error', 'Could not update model role'),
                            )
                    }
                }}>
                <input type="hidden" name="id" value={selectedModel.id} />
                <input type="hidden" name="role" value={selectedModelRole} />
                <div class="px-6 py-6">
                    <h3 class="mb-3 text-sm font-semibold">Model role</h3>
                    <RadioGroup.Root bind:value={selectedModelRole} class="gap-0">
                        <div class="rounded-xl border">
                            {#each roleOptions as option, index (option.value)}
                                {@const optionId = `model-role-${option.value}`}
                                <Label
                                    for={optionId}
                                    class={`flex cursor-pointer items-center gap-3 py-3 pr-6 pl-3 transition-colors ${selectedModelRole === option.value ? 'dark:bg-muted/50 bg-[#f4f3ef]' : 'dark:hover:bg-muted/40 hover:bg-[#faf9f6]'} ${index > 0 ? 'border-t' : 'rounded-t-xl'} ${index === roleOptions.length - 1 ? 'rounded-b-xl' : ''}`}>
                                    <RadioGroup.Item id={optionId} value={option.value} />
                                    <span>
                                        <span class="block text-sm font-medium"
                                            >{option.label}</span>
                                        <span class="text-muted-foreground block text-xs"
                                            >{option.hint}</span>
                                    </span>
                                </Label>
                            {/each}
                        </div>
                    </RadioGroup.Root>
                    <div
                        class="text-muted-foreground mt-6 flex items-center justify-between text-sm">
                        <span>Image input</span>
                        {#if data.visionByModelId?.[selectedModel.id]}
                            <span
                                class="inline-flex items-center gap-2 text-green-700 dark:text-green-400">
                                <Check class="h-4 w-4" />
                                Supported
                            </span>
                        {:else}
                            <span class="inline-flex items-center gap-2">
                                <X class="h-4 w-4" />
                                Not supported
                            </span>
                        {/if}
                    </div>
                </div>
            </form>
            <div class="flex items-center justify-between border-t px-6 py-5">
                <form
                    method="POST"
                    action="?/deleteModel"
                    use:enhance={() =>
                        async ({ result, update }) => {
                            await update()
                            confirmDialogOpen = false
                            if (result.type === 'success') {
                                modelDetailOpen = false
                                toast.success(
                                    actionMessage(result.data, 'message', 'Model deleted'),
                                )
                            } else if (result.type === 'failure')
                                toast.error(
                                    actionMessage(result.data, 'error', 'Could not remove model'),
                                )
                        }}>
                    <input type="hidden" name="id" value={selectedModel.id} />
                    <Button
                        type="button"
                        variant="ghost"
                        class="text-destructive hover:text-destructive -ml-4 cursor-pointer"
                        onclick={(event) =>
                            requestConfirm(
                                'Remove model',
                                `Are you sure you want to remove "${selectedModel?.displayName}"? Existing chats using this model will fall back to the default.`,
                                (event.currentTarget as HTMLElement).closest(
                                    'form',
                                ) as HTMLFormElement,
                            )}>Remove model</Button>
                </form>
                <div class="flex gap-3">
                    <Button
                        type="button"
                        variant="ghost"
                        class="cursor-pointer"
                        onclick={() => (modelDetailOpen = false)}>Cancel</Button>
                    <Button
                        type="submit"
                        form="model-role-form"
                        disabled={isRoleSubmitting}
                        class="cursor-pointer"
                        >{#if isRoleSubmitting}<Loader2
                                class="mr-2 h-4 w-4 animate-spin" />Saving...{:else}Save changes{/if}</Button>
                </div>
            </div>
        {/if}
    </Dialog.Content>
</Dialog.Root>

<!-- Add/discover model dialog -->
<Dialog.Root bind:open={modelDialogOpen}>
    <Dialog.Content class="sm:max-w-md">
        <Dialog.Header
            ><Dialog.Title>Add model</Dialog.Title><Dialog.Description
                >Add a model to this provider.</Dialog.Description
            ></Dialog.Header>
        <form
            method="POST"
            action="?/addModel"
            use:enhance={() => {
                isModelSubmitting = true
                return async ({ result, update }) => {
                    await update()
                    isModelSubmitting = false
                    if (result.type === 'success') {
                        modelDialogOpen = false
                        toast.success(
                            actionMessage(result.data, 'message', 'Model added successfully'),
                        )
                    } else if (result.type === 'failure')
                        toast.error(actionMessage(result.data, 'error', 'Something went wrong'))
                }
            }}
            class="space-y-4">
            <input type="hidden" name="providerId" value={modelFormState.providerId} />
            <div class="space-y-2">
                <Label for="model-id-picker">Model ID *</Label>
                <Popover.Root bind:open={modelPickerOpen}>
                    <Popover.Trigger
                        id="model-id-picker"
                        class="dark:bg-input/30 flex w-full cursor-pointer items-center justify-between gap-2 rounded-md border bg-transparent px-3 py-2 text-left text-sm">
                        {#if modelFormState.modelId}<span class="truncate font-mono text-sm"
                                >{modelFormState.modelId}</span
                            >{:else}<span class="text-muted-foreground">Select a model…</span
                            >{/if}<ChevronsUpDown class="h-4 w-4 shrink-0 opacity-50" />
                    </Popover.Trigger>
                    <Popover.Content class="w-[var(--bits-popover-anchor-width)] p-0" align="start">
                        <Command.Root shouldFilter={false}
                            ><Command.Input
                                placeholder="Search or enter a model ID…"
                                bind:value={modelSearch} /><Command.List
                                ><Command.Empty
                                    >{modelsLoading
                                        ? 'Loading model list…'
                                        : 'No models found.'}</Command.Empty
                                ><Command.Group
                                    >{#each filteredModels as option (option.modelId)}<Command.Item
                                            value={`${option.displayName} ${option.modelId}`}
                                            onSelect={() => applyModelChoice(option)}
                                            ><div class="flex flex-col py-0.5">
                                                <span class="text-sm">{option.displayName}</span
                                                ><span
                                                    class="text-muted-foreground font-mono text-xs"
                                                    >{option.modelId}</span>
                                            </div></Command.Item
                                        >{/each}</Command.Group
                                ></Command.List
                            >{#if modelSearch.trim()}<div class="border-t p-1">
                                    <Button
                                        variant="ghost"
                                        size="sm"
                                        type="button"
                                        class="w-full cursor-pointer justify-start"
                                        onclick={() => {
                                            modelFormState.modelId = modelSearch.trim()
                                            modelPickerOpen = false
                                        }}>Use custom model ID</Button>
                                </div>{/if}</Command.Root>
                    </Popover.Content>
                </Popover.Root>
                <p class="text-muted-foreground text-xs">
                    Search available models or enter any model ID to add it manually.
                </p>
                <input type="hidden" name="modelId" value={modelFormState.modelId} />
            </div>
            <div class="space-y-2">
                <Label for="model-display-name">Display Name *</Label><Input
                    id="model-display-name"
                    name="displayName"
                    bind:value={modelFormState.displayName}
                    placeholder="e.g., Claude Sonnet 4.5"
                    required />
            </div>
            <div class="space-y-2">
                <Label for="model-role">Role</Label><input
                    type="hidden"
                    name="role"
                    value={modelFormState.role} /><Select.Root
                    type="single"
                    bind:value={modelFormState.role}
                    ><Select.Trigger id="model-role" class="w-full cursor-pointer"
                        >{roleLabel(modelFormState.role)}</Select.Trigger
                    ><Select.Content
                        >{#each roleOptions as option (option.value)}<Select.Item
                                value={option.value}
                                class="cursor-pointer">{option.label}</Select.Item
                            >{/each}</Select.Content
                    ></Select.Root>
            </div>
            <Dialog.Footer
                ><Button
                    variant="outline"
                    type="button"
                    class="cursor-pointer"
                    onclick={() => (modelDialogOpen = false)}>Cancel</Button
                ><Button
                    type="submit"
                    disabled={isModelSubmitting || !modelFormState.modelId.trim()}
                    class="cursor-pointer"
                    >{#if isModelSubmitting}<Loader2
                            class="mr-2 h-4 w-4 animate-spin" />Adding...{:else}Add model{/if}</Button
                ></Dialog.Footer>
        </form>
    </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={confirmDialogOpen}>
    <AlertDialog.Content>
        <AlertDialog.Header
            ><AlertDialog.Title>{confirmTitle}</AlertDialog.Title><AlertDialog.Description
                >{confirmDescription}</AlertDialog.Description
            ></AlertDialog.Header>
        <AlertDialog.Footer
            ><AlertDialog.Cancel class="cursor-pointer">Cancel</AlertDialog.Cancel
            ><AlertDialog.Action
                class="bg-destructive text-destructive-foreground hover:bg-destructive/90 cursor-pointer"
                onclick={() => confirmFormRef?.requestSubmit()}>Remove</AlertDialog.Action
            ></AlertDialog.Footer>
    </AlertDialog.Content>
</AlertDialog.Root>
