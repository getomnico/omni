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

    // Form state with defaults
    let provider = $state<'jina' | 'bedrock'>(data.config?.provider || 'jina')
    let jinaApiKey = $state('')
    let jinaModel = $state(data.config?.jinaModel || 'jina-embeddings-v3')
    let jinaApiUrl = $state(data.config?.jinaApiUrl || 'https://api.jina.ai/v1/embeddings')
    let bedrockModelId = $state(data.config?.bedrockModelId || 'amazon.titan-embed-text-v2:0')
    let isSubmitting = $state(false)

    // Model suggestions based on provider
    const modelSuggestions = {
        jina: ['jina-embeddings-v3', 'jina-embeddings-v2-base-en'],
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
                                <RadioGroup.Item value="jina" id="jina" />
                                <Label for="jina" class="font-normal">Jina AI (Cloud API)</Label>
                            </div>
                            <div class="flex items-center space-x-2">
                                <RadioGroup.Item value="bedrock" id="bedrock" />
                                <Label for="bedrock" class="font-normal">AWS Bedrock</Label>
                            </div>
                        </RadioGroup.Root>
                    </Card.Content>
                </Card.Root>

                <!-- Provider-specific Configuration -->
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
                                <Label for="jinaApiKey"
                                    >API Key {data.hasJinaApiKey ? '' : '*'}</Label>
                                <Input
                                    id="jinaApiKey"
                                    name="jinaApiKey"
                                    type="password"
                                    bind:value={jinaApiKey}
                                    placeholder={data.hasJinaApiKey
                                        ? 'Leave empty to keep current key'
                                        : 'jina_...'}
                                    required={provider === 'jina' && !data.hasJinaApiKey} />
                                <p class="text-muted-foreground text-sm">
                                    {data.hasJinaApiKey
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
