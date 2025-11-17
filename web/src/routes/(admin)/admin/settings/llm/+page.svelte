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
    let provider = $state<'vllm' | 'anthropic' | 'bedrock'>(data.config?.provider || 'anthropic')
    let primaryModelId = $state(data.config?.primaryModelId || '')
    let secondaryModelId = $state(data.config?.secondaryModelId || '')
    let vllmUrl = $state(data.config?.vllmUrl || 'http://vllm:8000')
    let anthropicApiKey = $state('')
    let maxTokens = $state<string>(data.config?.maxTokens?.toString() || '')
    let temperature = $state<string>(data.config?.temperature?.toString() || '')
    let topP = $state<string>(data.config?.topP?.toString() || '')
    let isSubmitting = $state(false)

    // Common model suggestions based on provider
    const modelSuggestions = {
        vllm: ['meta-llama/Llama-3.1-8B-Instruct', 'mistralai/Mistral-7B-Instruct-v0.3'],
        anthropic: [
            'claude-sonnet-4-20250514',
            'claude-opus-4-20250514',
            'claude-haiku-4-20250312',
        ],
        bedrock: [
            'us.anthropic.claude-sonnet-4-20250514-v1:0',
            'us.anthropic.claude-haiku-4-5-20251001-v1:0',
            'amazon.nova-pro-v1:0',
            'amazon.nova-lite-v1:0',
        ],
    }
</script>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">LLM Configuration</h1>
            <p class="text-muted-foreground mt-2">
                Configure the large language model provider for AI-powered features
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
                    No LLM configuration found in database. The system will use environment
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
                        <Card.Description>Select the LLM provider you want to use</Card.Description>
                    </Card.Header>
                    <Card.Content>
                        <RadioGroup.Root bind:value={provider} name="provider" class="space-y-3">
                            <div class="flex items-center space-x-2">
                                <RadioGroup.Item value="vllm" id="vllm" />
                                <Label for="vllm" class="font-normal">vLLM (Self-hosted)</Label>
                            </div>
                            <div class="flex items-center space-x-2">
                                <RadioGroup.Item value="anthropic" id="anthropic" />
                                <Label for="anthropic" class="font-normal">
                                    Anthropic Claude (API)
                                </Label>
                            </div>
                            <div class="flex items-center space-x-2">
                                <RadioGroup.Item value="bedrock" id="bedrock" />
                                <Label for="bedrock" class="font-normal">AWS Bedrock</Label>
                            </div>
                        </RadioGroup.Root>
                    </Card.Content>
                </Card.Root>

                <!-- Provider-specific Configuration -->
                {#if provider === 'vllm'}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>vLLM Configuration</Card.Title>
                            <Card.Description>
                                Configure connection to your self-hosted vLLM instance
                            </Card.Description>
                        </Card.Header>
                        <Card.Content class="space-y-4">
                            <div class="space-y-2">
                                <Label for="vllmUrl">vLLM URL *</Label>
                                <Input
                                    id="vllmUrl"
                                    name="vllmUrl"
                                    bind:value={vllmUrl}
                                    placeholder="http://vllm:8000"
                                    required={provider === 'vllm'} />
                                <p class="text-muted-foreground text-sm">
                                    The URL of your vLLM service (OpenAI-compatible API)
                                </p>
                            </div>
                        </Card.Content>
                    </Card.Root>
                {/if}

                {#if provider === 'anthropic'}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>Anthropic Configuration</Card.Title>
                            <Card.Description>
                                Configure API access to Anthropic Claude
                            </Card.Description>
                        </Card.Header>
                        <Card.Content class="space-y-4">
                            <div class="space-y-2">
                                <Label for="anthropicApiKey"
                                    >API Key {data.hasAnthropicApiKey ? '' : '*'}</Label>
                                <Input
                                    id="anthropicApiKey"
                                    name="anthropicApiKey"
                                    type="password"
                                    bind:value={anthropicApiKey}
                                    placeholder={data.hasAnthropicApiKey
                                        ? 'Leave empty to keep current key'
                                        : 'sk-ant-...'}
                                    required={provider === 'anthropic' &&
                                        !data.hasAnthropicApiKey} />
                                <p class="text-muted-foreground text-sm">
                                    {data.hasAnthropicApiKey
                                        ? 'Leave empty to keep current key, or enter new key to update'
                                        : 'Your Anthropic API key (starts with sk-ant-)'}
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
                        <Card.Content>
                            <Alert.Root>
                                <Info class="h-4 w-4" />
                                <Alert.Description>
                                    Ensure your application has appropriate IAM permissions to
                                    invoke Bedrock models
                                </Alert.Description>
                            </Alert.Root>
                        </Card.Content>
                    </Card.Root>
                {/if}

                <!-- Model Configuration -->
                <Card.Root>
                    <Card.Header>
                        <Card.Title>Model Configuration</Card.Title>
                        <Card.Description>
                            Specify the models to use for different tasks
                        </Card.Description>
                    </Card.Header>
                    <Card.Content class="space-y-4">
                        <div class="space-y-2">
                            <Label for="primaryModelId">Primary Model *</Label>
                            <Input
                                id="primaryModelId"
                                name="primaryModelId"
                                bind:value={primaryModelId}
                                placeholder={modelSuggestions[provider][0]}
                                required />
                            <p class="text-muted-foreground text-sm">
                                Used for main AI tasks (chat, search answers, etc.)
                            </p>
                            <div class="text-muted-foreground text-xs">
                                <p class="mb-1 font-medium">Common models for {provider}:</p>
                                <ul class="list-inside list-disc space-y-0.5">
                                    {#each modelSuggestions[provider] as suggestion}
                                        <li>{suggestion}</li>
                                    {/each}
                                </ul>
                            </div>
                        </div>

                        <div class="space-y-2">
                            <Label for="secondaryModelId">Secondary Model (Optional)</Label>
                            <Input
                                id="secondaryModelId"
                                name="secondaryModelId"
                                bind:value={secondaryModelId}
                                placeholder="e.g., {modelSuggestions[provider][1] ||
                                    'Faster model for simple tasks'}" />
                            <p class="text-muted-foreground text-sm">
                                Used for smaller tasks like title generation (leave empty to use
                                primary model)
                            </p>
                        </div>
                    </Card.Content>
                </Card.Root>

                <!-- Advanced Parameters -->
                <Card.Root>
                    <Card.Header>
                        <Card.Title>Advanced Parameters</Card.Title>
                        <Card.Description>
                            Optional parameters to control LLM behavior (leave empty for defaults)
                        </Card.Description>
                    </Card.Header>
                    <Card.Content class="space-y-4">
                        <div class="grid grid-cols-1 gap-4 md:grid-cols-3">
                            <div class="space-y-2">
                                <Label for="maxTokens">Max Tokens</Label>
                                <Input
                                    id="maxTokens"
                                    name="maxTokens"
                                    type="number"
                                    bind:value={maxTokens}
                                    placeholder="4096"
                                    min="1" />
                                <p class="text-muted-foreground text-xs">Maximum response length</p>
                            </div>

                            <div class="space-y-2">
                                <Label for="temperature">Temperature</Label>
                                <Input
                                    id="temperature"
                                    name="temperature"
                                    type="number"
                                    bind:value={temperature}
                                    placeholder="0.0"
                                    min="0"
                                    max="2"
                                    step="0.1" />
                                <p class="text-muted-foreground text-xs">Randomness (0.0-2.0)</p>
                            </div>

                            <div class="space-y-2">
                                <Label for="topP">Top P</Label>
                                <Input
                                    id="topP"
                                    name="topP"
                                    type="number"
                                    bind:value={topP}
                                    placeholder="1.0"
                                    min="0"
                                    max="1"
                                    step="0.1" />
                                <p class="text-muted-foreground text-xs">
                                    Nucleus sampling (0.0-1.0)
                                </p>
                            </div>
                        </div>
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
