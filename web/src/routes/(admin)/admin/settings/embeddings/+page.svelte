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
    // Local fields
    let localBaseUrl = $state(data.config?.localBaseUrl || 'http://embeddings:8001/v1')
    let localModel = $state(data.config?.localModel || 'nomic-ai/nomic-embed-text-v1.5')
    // Jina fields
    let jinaApiKey = $state('')
    let jinaModel = $state(data.config?.jinaModel || 'jina-embeddings-v3')
    let jinaApiUrl = $state(data.config?.jinaApiUrl || 'https://api.jina.ai/v1/embeddings')
    // OpenAI fields
    let openaiApiKey = $state('')
    let openaiModel = $state(data.config?.openaiModel || 'text-embedding-3-small')
    let openaiDimensions = $state(data.config?.openaiDimensions || 1536)
    // Cohere fields
    let cohereApiKey = $state('')
    let cohereModel = $state(data.config?.cohereModel || 'embed-v4.0')
    let cohereApiUrl = $state(data.config?.cohereApiUrl || 'https://api.cohere.com/v2/embed')
    let cohereDimensions = $state(data.config?.cohereDimensions || '')
    // Bedrock fields
    let bedrockModelId = $state(data.config?.bedrockModelId || 'amazon.titan-embed-text-v2:0')
    // Form state
    let isSubmitting = $state(false)

    // Model suggestions based on provider
    const modelSuggestions = {
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

                <!-- Local Configuration -->
                {#if provider === 'local'}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>Local Embedding Server Configuration</Card.Title>
                            <Card.Description>
                                Configure connection to a self-hosted embedding server (e.g., vLLM
                                serving embedding models)
                            </Card.Description>
                        </Card.Header>
                        <Card.Content class="space-y-4">
                            <div class="space-y-2">
                                <Label for="localBaseUrl">Base URL *</Label>
                                <Input
                                    id="localBaseUrl"
                                    name="localBaseUrl"
                                    bind:value={localBaseUrl}
                                    placeholder="http://embeddings:8001/v1"
                                    required={provider === 'local'} />
                                <p class="text-muted-foreground text-sm">
                                    URL of your local embedding server (OpenAI-compatible API)
                                </p>
                            </div>

                            <div class="space-y-2">
                                <Label for="localModel">Model *</Label>
                                <Input
                                    id="localModel"
                                    name="localModel"
                                    bind:value={localModel}
                                    placeholder="nomic-ai/nomic-embed-text-v1.5"
                                    required={provider === 'local'} />
                                <p class="text-muted-foreground text-sm">
                                    Model name as configured on your embedding server
                                </p>
                                <div class="text-muted-foreground text-xs">
                                    <p class="mb-1 font-medium">Common models:</p>
                                    <ul class="list-inside list-disc space-y-0.5">
                                        {#each modelSuggestions.local as suggestion}
                                            <li>{suggestion}</li>
                                        {/each}
                                    </ul>
                                </div>
                            </div>
                        </Card.Content>
                    </Card.Root>
                {/if}

                <!-- Jina Configuration -->
                {#if provider === 'jina'}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>Jina AI Configuration</Card.Title>
                            <Card.Description>
                                Configure connection to Jina AI embedding API
                            </Card.Description>
                        </Card.Header>
                        <Card.Content class="space-y-4">
                            <div class="space-y-2">
                                <Label for="jinaApiKey">API Key {data.hasApiKey ? '' : '*'}</Label>
                                <Input
                                    id="jinaApiKey"
                                    name="jinaApiKey"
                                    type="password"
                                    bind:value={jinaApiKey}
                                    placeholder={data.hasApiKey
                                        ? 'Leave empty to keep current key'
                                        : 'jina_...'}
                                    required={provider === 'jina' && !data.hasApiKey} />
                                <p class="text-muted-foreground text-sm">
                                    {data.hasApiKey
                                        ? 'Leave empty to keep current key, or enter new key to update'
                                        : 'Your Jina AI API key'}
                                </p>
                            </div>

                            <div class="space-y-2">
                                <Label for="jinaModel">Model *</Label>
                                <Input
                                    id="jinaModel"
                                    name="jinaModel"
                                    bind:value={jinaModel}
                                    placeholder="jina-embeddings-v3"
                                    required={provider === 'jina'} />
                                <p class="text-muted-foreground text-sm">
                                    Jina embedding model to use
                                </p>
                                <div class="text-muted-foreground text-xs">
                                    <p class="mb-1 font-medium">Available models:</p>
                                    <ul class="list-inside list-disc space-y-0.5">
                                        {#each modelSuggestions.jina as suggestion}
                                            <li>{suggestion}</li>
                                        {/each}
                                    </ul>
                                </div>
                            </div>

                            <div class="space-y-2">
                                <Label for="jinaApiUrl">API URL</Label>
                                <Input
                                    id="jinaApiUrl"
                                    name="jinaApiUrl"
                                    bind:value={jinaApiUrl}
                                    placeholder="https://api.jina.ai/v1/embeddings" />
                                <p class="text-muted-foreground text-sm">
                                    Jina AI API endpoint (leave default unless using custom
                                    endpoint)
                                </p>
                            </div>
                        </Card.Content>
                    </Card.Root>
                {/if}

                <!-- OpenAI Configuration -->
                {#if provider === 'openai'}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>OpenAI Configuration</Card.Title>
                            <Card.Description>
                                Configure connection to OpenAI embedding API
                            </Card.Description>
                        </Card.Header>
                        <Card.Content class="space-y-4">
                            <div class="space-y-2">
                                <Label for="openaiApiKey"
                                    >API Key {data.hasApiKey ? '' : '*'}</Label>
                                <Input
                                    id="openaiApiKey"
                                    name="openaiApiKey"
                                    type="password"
                                    bind:value={openaiApiKey}
                                    placeholder={data.hasApiKey
                                        ? 'Leave empty to keep current key'
                                        : 'sk-...'}
                                    required={provider === 'openai' && !data.hasApiKey} />
                                <p class="text-muted-foreground text-sm">
                                    {data.hasApiKey
                                        ? 'Leave empty to keep current key, or enter new key to update'
                                        : 'Your OpenAI API key'}
                                </p>
                            </div>

                            <div class="space-y-2">
                                <Label for="openaiModel">Model *</Label>
                                <Input
                                    id="openaiModel"
                                    name="openaiModel"
                                    bind:value={openaiModel}
                                    placeholder="text-embedding-3-small"
                                    required={provider === 'openai'} />
                                <p class="text-muted-foreground text-sm">
                                    OpenAI embedding model to use
                                </p>
                                <div class="text-muted-foreground text-xs">
                                    <p class="mb-1 font-medium">Available models:</p>
                                    <ul class="list-inside list-disc space-y-0.5">
                                        {#each modelSuggestions.openai as suggestion}
                                            <li>{suggestion}</li>
                                        {/each}
                                    </ul>
                                </div>
                            </div>

                            <div class="space-y-2">
                                <Label for="openaiDimensions">Dimensions</Label>
                                <Input
                                    id="openaiDimensions"
                                    name="openaiDimensions"
                                    type="number"
                                    bind:value={openaiDimensions}
                                    placeholder="1536"
                                    min="1"
                                    max="3072" />
                                <p class="text-muted-foreground text-sm">
                                    Embedding dimensions (text-embedding-3-* supports 256-3072,
                                    ada-002 is fixed at 1536)
                                </p>
                            </div>
                        </Card.Content>
                    </Card.Root>
                {/if}

                <!-- Cohere Configuration -->
                {#if provider === 'cohere'}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>Cohere Configuration</Card.Title>
                            <Card.Description>
                                Configure connection to Cohere embedding API
                            </Card.Description>
                        </Card.Header>
                        <Card.Content class="space-y-4">
                            <div class="space-y-2">
                                <Label for="cohereApiKey"
                                    >API Key {data.hasApiKey ? '' : '*'}</Label>
                                <Input
                                    id="cohereApiKey"
                                    name="cohereApiKey"
                                    type="password"
                                    bind:value={cohereApiKey}
                                    placeholder={data.hasApiKey
                                        ? 'Leave empty to keep current key'
                                        : 'Enter Cohere API key'}
                                    required={provider === 'cohere' && !data.hasApiKey} />
                                <p class="text-muted-foreground text-sm">
                                    {data.hasApiKey
                                        ? 'Leave empty to keep current key, or enter new key to update'
                                        : 'Your Cohere API key'}
                                </p>
                            </div>

                            <div class="space-y-2">
                                <Label for="cohereModel">Model *</Label>
                                <Input
                                    id="cohereModel"
                                    name="cohereModel"
                                    bind:value={cohereModel}
                                    placeholder="embed-v4.0"
                                    required={provider === 'cohere'} />
                                <p class="text-muted-foreground text-sm">
                                    Cohere embedding model to use
                                </p>
                                <div class="text-muted-foreground text-xs">
                                    <p class="mb-1 font-medium">Available models:</p>
                                    <ul class="list-inside list-disc space-y-0.5">
                                        {#each modelSuggestions.cohere as suggestion}
                                            <li>{suggestion}</li>
                                        {/each}
                                    </ul>
                                </div>
                            </div>

                            <div class="space-y-2">
                                <Label for="cohereApiUrl">API URL</Label>
                                <Input
                                    id="cohereApiUrl"
                                    name="cohereApiUrl"
                                    bind:value={cohereApiUrl}
                                    placeholder="https://api.cohere.com/v2/embed" />
                                <p class="text-muted-foreground text-sm">
                                    Cohere API endpoint (leave default unless using custom endpoint)
                                </p>
                            </div>

                            <div class="space-y-2">
                                <Label for="cohereDimensions">Dimensions</Label>
                                <Input
                                    id="cohereDimensions"
                                    name="cohereDimensions"
                                    type="number"
                                    bind:value={cohereDimensions}
                                    placeholder="Leave empty for model default"
                                    min="1"
                                    max="4096" />
                                <p class="text-muted-foreground text-sm">
                                    Output embedding dimensions (embed-v4.0 supports 256, 512, 1024,
                                    1536)
                                </p>
                            </div>
                        </Card.Content>
                    </Card.Root>
                {/if}

                <!-- Bedrock Configuration -->
                {#if provider === 'bedrock'}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>AWS Bedrock Configuration</Card.Title>
                            <Card.Description>
                                AWS Bedrock uses IAM roles for authentication. Region is
                                auto-detected from environment.
                            </Card.Description>
                        </Card.Header>
                        <Card.Content class="space-y-4">
                            <div class="space-y-2">
                                <Label for="bedrockModelId">Model ID *</Label>
                                <Input
                                    id="bedrockModelId"
                                    name="bedrockModelId"
                                    bind:value={bedrockModelId}
                                    placeholder="amazon.titan-embed-text-v2:0"
                                    required={provider === 'bedrock'} />
                                <p class="text-muted-foreground text-sm">
                                    Bedrock embedding model identifier
                                </p>
                                <div class="text-muted-foreground text-xs">
                                    <p class="mb-1 font-medium">Common models:</p>
                                    <ul class="list-inside list-disc space-y-0.5">
                                        {#each modelSuggestions.bedrock as suggestion}
                                            <li>{suggestion}</li>
                                        {/each}
                                    </ul>
                                </div>
                            </div>

                            <Alert.Root>
                                <Info class="h-4 w-4" />
                                <Alert.Description>
                                    Ensure your application has appropriate IAM permissions to
                                    invoke Bedrock embedding models
                                </Alert.Description>
                            </Alert.Root>
                        </Card.Content>
                    </Card.Root>
                {/if}

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
