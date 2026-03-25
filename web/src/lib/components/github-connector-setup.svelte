<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { AuthType, type GitHubSourceConfig, type GitHubCredentials } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = $bindable(false), onSuccess, onCancel }: Props = $props()

    let token = $state('')
    let apiUrl = $state('')
    let includeDiscussions = $state(true)
    let includeForks = $state(false)
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!token.trim()) {
                throw new Error('Personal access token is required')
            }

            const config: GitHubSourceConfig = {
                include_discussions: includeDiscussions,
                include_forks: includeForks,
                ...(apiUrl.trim() ? { api_url: apiUrl.trim() } : {}),
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'GitHub',
                    sourceType: 'github',
                    config,
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create GitHub source')
            }

            const source = await sourceResponse.json()

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
            open = false

            token = ''
            apiUrl = ''
            includeDiscussions = true
            includeForks = false

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up GitHub:', error)
            toast.error(error.message || 'Failed to set up GitHub')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        open = false
        token = ''
        apiUrl = ''
        includeDiscussions = true
        includeForks = false
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect GitHub</Dialog.Title>
            <Dialog.Description>
                Set up your GitHub integration to index repositories, issues, PRs, and discussions.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
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
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                {isSubmitting ? 'Connecting...' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
