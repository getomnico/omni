<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { AlertCircle, CheckCircle2, Loader2, Info } from '@lucide/svelte'
    import type { PageData, ActionData } from './$types'

    let { data, form }: { data: PageData; form: ActionData } = $props()

    type Provider = 'local' | 'jina' | 'openai' | 'cohere' | 'bedrock'

    // Form state with defaults
    let provider = $state<Provider>(data.config?.provider || 'jina')
    let model = $state(data.config?.model || '')
    let apiKey = $state('')
    let apiUrl = $state(data.config?.apiUrl || '')
    let dimensions = $state<number | string>(data.config?.dimensions || '')
    let isSubmitting = $state(false)

    // Provider-specific defaults for model and apiUrl
    const providerDefaults: Record<Provider, { model: string; apiUrl: string }> = {
        local: { model: 'nomic-ai/nomic-embed-text-v1.5', apiUrl: 'http://embeddings:8001/v1' },
        jina: { model: 'jina-embeddings-v3', apiUrl: 'https://api.jina.ai/v1/embeddings' },
        openai: { model: 'text-embedding-3-small', apiUrl: '' },
        cohere: { model: 'embed-v4.0', apiUrl: 'https://api.cohere.com/v2/embed' },
        bedrock: { model: 'amazon.titan-embed-text-v2:0', apiUrl: '' },
    }

    // Set defaults when provider changes (only if no saved config for this provider)
    $effect(() => {
        if (data.config?.provider !== provider) {
            const defaults = providerDefaults[provider]
            model = defaults.model
            apiUrl = defaults.apiUrl
        }
    })

    // Which fields to show per provider
    const showApiKey = (p: Provider) => ['jina', 'openai', 'cohere'].includes(p)
    const showApiUrl = (p: Provider) => ['local', 'jina', 'cohere'].includes(p)
    const showDimensions = (p: Provider) => ['openai', 'cohere'].includes(p)

    // Model suggestions based on provider
    const modelSuggestions: Record<Provider, string[]> = {
        local: [
            'intfloat/e5-large-v2',
            'BAAI/bge-large-en-v1.5',
            'sentence-transformers/all-MiniLM-L6-v2',
        ],
        jina: ['jina-embeddings-v3', 'jina-embeddings-v2-base-en'],
        openai: ['text-embedding-3-small', 'text-embedding-3-large', 'text-embedding-ada-002'],
        cohere: ['embed-v4.0', 'embed-v3.0', 'embed-multilingual-v3.0'],
        bedrock: [
            'amazon.titan-embed-text-v2:0',
            'amazon.titan-embed-text-v1',
            'cohere.embed-english-v3',
            'cohere.embed-multilingual-v3',
        ],
    }
</script>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Embedding Configuration</h1>
            <p class="text-muted-foreground mt-2">
                Configure the embedding provider for semantic search and document processing
            </p>
        </div>

        {#if form?.success}
            <Alert.Root variant="default" class="border-green-500 bg-green-50">
                <CheckCircle2 class="h-4 w-4 text-green-600" />
                <Alert.Title class="text-green-900">Success</Alert.Title>
                <Alert.Description class="text-green-800">
                    {form.message || 'Configuration saved successfully'}
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

        {#if !data.config}
            <Alert.Root>
                <Info class="h-4 w-4" />
                <Alert.Title>No Configuration Found</Alert.Title>
                <Alert.Description>
                    No embedding configuration found in database. The system will use environment
                    variables if configured. Save a configuration here to override.
                </Alert.Description>
            </Alert.Root>
        {/if}

        <form
            method="POST"
            action="?/save"
            use:enhance={() => {
                isSubmitting = true
                return async ({ update }) => {
                    await update()
                    isSubmitting = false
                }
            }}>
            <div class="space-y-6">
                <!-- Provider Selection -->
                <Card.Root>
                    <Card.Header>
                        <Card.Title>Provider</Card.Title>
                        <Card.Description>
                            Select the embedding provider you want to use
                        </Card.Description>
                    </Card.Header>
                    <Card.Content>
                        <RadioGroup.Root bind:value={provider} name="provider" class="space-y-3">
                            <div class="flex items-center space-x-2">
                                <RadioGroup.Item value="local" id="local" />
                                <Label for="local" class="font-normal"
                                    >Local (Self-hosted via vLLM)</Label>
                            </div>
                            <div class="flex items-center space-x-2">
                                <RadioGroup.Item value="jina" id="jina" />
                                <Label for="jina" class="font-normal">Jina AI (Cloud API)</Label>
                            </div>
                            <div class="flex items-center space-x-2">
                                <RadioGroup.Item value="openai" id="openai" />
                                <Label for="openai" class="font-normal">OpenAI</Label>
                            </div>
                            <div class="flex items-center space-x-2">
                                <RadioGroup.Item value="cohere" id="cohere" />
                                <Label for="cohere" class="font-normal">Cohere</Label>
                            </div>
                            <div class="flex items-center space-x-2">
                                <RadioGroup.Item value="bedrock" id="bedrock" />
                                <Label for="bedrock" class="font-normal">AWS Bedrock</Label>
                            </div>
                        </RadioGroup.Root>
                    </Card.Content>
                </Card.Root>

                <!-- Provider Configuration -->
                <Card.Root>
                    <Card.Header>
                        <Card.Title>Configuration</Card.Title>
                        <Card.Description>
                            {#if provider === 'local'}
                                Configure connection to a self-hosted embedding server (e.g., vLLM
                                serving embedding models)
                            {:else if provider === 'bedrock'}
                                AWS Bedrock uses IAM roles for authentication. Region is
                                auto-detected from environment.
                            {:else}
                                Configure connection to the {provider} embedding API
                            {/if}
                        </Card.Description>
                    </Card.Header>
                    <Card.Content class="space-y-4">
                        <!-- API Key (jina, openai, cohere) -->
                        {#if showApiKey(provider)}
                            <div class="space-y-2">
                                <Label for="apiKey">API Key {data.hasApiKey ? '' : '*'}</Label>
                                <Input
                                    id="apiKey"
                                    name="apiKey"
                                    type="password"
                                    bind:value={apiKey}
                                    placeholder={data.hasApiKey
                                        ? 'Leave empty to keep current key'
                                        : 'Enter API key'}
                                    required={showApiKey(provider) && !data.hasApiKey} />
                                <p class="text-muted-foreground text-sm">
                                    {data.hasApiKey
                                        ? 'Leave empty to keep current key, or enter new key to update'
                                        : `Your ${provider} API key`}
                                </p>
                            </div>
                        {/if}

                        <!-- Model (always shown) -->
                        <div class="space-y-2">
                            <Label for="model">Model *</Label>
                            <Input
                                id="model"
                                name="model"
                                bind:value={model}
                                placeholder={providerDefaults[provider].model}
                                required />
                            <p class="text-muted-foreground text-sm">Embedding model to use</p>
                            <div class="text-muted-foreground text-xs">
                                <p class="mb-1 font-medium">Common models:</p>
                                <ul class="list-inside list-disc space-y-0.5">
                                    {#each modelSuggestions[provider] as suggestion}
                                        <li>{suggestion}</li>
                                    {/each}
                                </ul>
                            </div>
                        </div>

                        <!-- API URL (local, jina, cohere) -->
                        {#if showApiUrl(provider)}
                            <div class="space-y-2">
                                <Label for="apiUrl"
                                    >API URL {provider === 'local' ? '*' : ''}</Label>
                                <Input
                                    id="apiUrl"
                                    name="apiUrl"
                                    bind:value={apiUrl}
                                    placeholder={providerDefaults[provider].apiUrl}
                                    required={provider === 'local'} />
                                <p class="text-muted-foreground text-sm">
                                    {#if provider === 'local'}
                                        URL of your local embedding server (OpenAI-compatible API)
                                    {:else}
                                        API endpoint (leave default unless using custom endpoint)
                                    {/if}
                                </p>
                            </div>
                        {/if}

                        <!-- Dimensions (openai, cohere) -->
                        {#if showDimensions(provider)}
                            <div class="space-y-2">
                                <Label for="dimensions">Dimensions</Label>
                                <Input
                                    id="dimensions"
                                    name="dimensions"
                                    type="number"
                                    bind:value={dimensions}
                                    placeholder="Leave empty for model default"
                                    min="1"
                                    max="4096" />
                                <p class="text-muted-foreground text-sm">
                                    Output embedding dimensions
                                </p>
                            </div>
                        {/if}

                        <!-- Bedrock IAM notice -->
                        {#if provider === 'bedrock'}
                            <Alert.Root>
                                <Info class="h-4 w-4" />
                                <Alert.Description>
                                    Ensure your application has appropriate IAM permissions to
                                    invoke Bedrock embedding models
                                </Alert.Description>
                            </Alert.Root>
                        {/if}
                    </Card.Content>
                </Card.Root>

                <!-- Submit Button -->
                <div class="flex justify-end">
                    <Button type="submit" disabled={isSubmitting} class="min-w-32 cursor-pointer">
                        {#if isSubmitting}
                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                            Saving...
                        {:else}
                            Save Configuration
                        {/if}
                    </Button>
                </div>
            </div>
        </form>
    </div>
</div>
