<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { AuthType, SourceType, type GitHubSourceConfig, type GitHubCredentials } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    // 'oauth' drives the per-user GitHub OAuth app; 'pat' is the legacy
    // personal-access-token path (useful when the OAuth client is not yet
    // configured or for GitHub Enterprise servers without OAuth apps).
    let authMode = $state<'oauth' | 'pat'>('oauth')
    let token = $state('')
    let apiUrl = $state('')
    let includeDiscussions = $state(true)
    let includeForks = $state(false)
    let readOnly = $state(false)
    let isSubmitting = $state(false)

    async function createSource(): Promise<{ id: string }> {
        const config: GitHubSourceConfig = {
            include_discussions: includeDiscussions,
            include_forks: includeForks,
            read_only: readOnly,
            ...(apiUrl.trim() ? { api_url: apiUrl.trim() } : {}),
        }
        const response = await fetch('/api/sources', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                scope: 'org',
                name: 'GitHub',
                sourceType: SourceType.GITHUB,
                config,
            }),
        })
        if (!response.ok) {
            throw new Error('Failed to create GitHub source')
        }
        return (await response.json()) as { id: string }
    }

    async function handleOAuthConnect() {
        isSubmitting = true
        try {
            const source = await createSource()
            toast.success('GitHub source created. Continue with GitHub to authorize access.')
            const returnTo = encodeURIComponent('/admin/settings/integrations?success=connected')
            window.location.href = `/api/oauth/start?source_id=${source.id}&flow=org_source&return_to=${returnTo}`
        } catch (error: unknown) {
            console.error('Error setting up GitHub:', error)
            toast.error(error instanceof Error ? error.message : 'Failed to set up GitHub')
            isSubmitting = false
        }
    }

    async function handlePatConnect() {
        isSubmitting = true
        try {
            if (!token.trim()) {
                throw new Error('Personal access token is required')
            }

            const source = await createSource()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'github',
                    authType: AuthType.BEARER_TOKEN,
                    credentials: { token } satisfies GitHubCredentials,
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create GitHub service credentials')
            }

            toast.success('GitHub connected successfully!')

            reset()
            if (onSuccess) {
                onSuccess()
            }
        } catch (error: unknown) {
            console.error('Error setting up GitHub:', error)
            toast.error(error instanceof Error ? error.message : 'Failed to set up GitHub')
        } finally {
            isSubmitting = false
        }
    }

    function handleSubmit() {
        if (authMode === 'oauth') {
            void handleOAuthConnect()
        } else {
            void handlePatConnect()
        }
    }

    function reset() {
        token = ''
        apiUrl = ''
        includeDiscussions = true
        includeForks = false
        readOnly = false
    }

    function handleCancel() {
        reset()
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect GitHub</Dialog.Title>
            <Dialog.Description>
                Set up your GitHub integration to index repositories, issues, PRs, and discussions.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="flex gap-2">
                <Button
                    type="button"
                    variant={authMode === 'oauth' ? 'default' : 'outline'}
                    class="cursor-pointer"
                    onclick={() => (authMode = 'oauth')}>
                    Connect with GitHub (recommended)
                </Button>
                <Button
                    type="button"
                    variant={authMode === 'pat' ? 'default' : 'outline'}
                    class="cursor-pointer"
                    onclick={() => (authMode = 'pat')}>
                    Personal access token
                </Button>
            </div>

            {#if authMode === 'oauth'}
                <div class="space-y-2">
                    <p class="text-muted-foreground text-sm">
                        Creates the source, then redirects to GitHub to authorize it with the
                        organization's OAuth app. The OAuth client must be configured under
                        Admin &rarr; Settings &rarr; Integrations &rarr; OAuth Apps
                        (requires <code>repo</code> and <code>read:org</code> scopes).
                    </p>
                </div>
            {:else}
                <div class="space-y-2">
                    <Label for="github-token">Personal Access Token</Label>
                    <Input
                        id="github-token"
                        bind:value={token}
                        placeholder="ghp_..."
                        type="password"
                        required />
                    <p class="text-muted-foreground text-sm">
                        Create a token at
                        <a
                            href="https://github.com/settings/tokens"
                            target="_blank"
                            class="text-blue-600 hover:underline"
                            >GitHub Settings &rarr; Developer settings &rarr; Personal access tokens</a
                        >. Requires <code>repo</code> and <code>read:org</code> scopes.
                    </p>
                </div>
            {/if}

            <div class="space-y-2">
                <Label for="github-api-url">API URL (optional)</Label>
                <Input
                    id="github-api-url"
                    bind:value={apiUrl}
                    placeholder="https://api.github.com" />
                <p class="text-muted-foreground text-sm">
                    Only needed for GitHub Enterprise. Leave blank for github.com.
                </p>
            </div>

            <div class="flex items-center gap-2">
                <Checkbox
                    id="include-discussions"
                    bind:checked={includeDiscussions}
                    class="cursor-pointer" />
                <Label for="include-discussions" class="cursor-pointer">Include Discussions</Label>
                <p class="text-muted-foreground text-sm">
                    Index GitHub Discussions in addition to issues and PRs
                </p>
            </div>

            <div class="flex items-center gap-2">
                <Checkbox id="include-forks" bind:checked={includeForks} class="cursor-pointer" />
                <Label for="include-forks" class="cursor-pointer">Include Forks</Label>
                <p class="text-muted-foreground text-sm">Also index forked repositories</p>
            </div>

            <div class="flex items-center gap-2">
                <Checkbox id="read-only" bind:checked={readOnly} class="cursor-pointer" />
                <Label for="read-only" class="cursor-pointer">Read-only mode</Label>
                <p class="text-muted-foreground text-sm">
                    Only allow read operations (no creating issues, PRs, etc.)
                </p>
            </div>
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                {#if isSubmitting}
                    Connecting...
                {:else if authMode === 'oauth'}
                    Continue with GitHub
                {:else}
                    Connect
                {/if}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
