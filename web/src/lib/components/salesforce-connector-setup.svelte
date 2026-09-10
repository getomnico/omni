<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Textarea } from '$lib/components/ui/textarea'
    import * as Tabs from '$lib/components/ui/tabs'
    import { AuthType } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    type AuthMode = 'jwt' | 'token'
    let authMode = $state<AuthMode>('jwt')

    // Connected App (JWT) fields
    let consumerKey = $state('')
    let privateKey = $state('')
    let username = $state('')
    let loginUrl = $state('https://login.salesforce.com')

    // Static token fields
    let instanceUrl = $state('')
    let accessToken = $state('')

    let isSubmitting = $state(false)

    function normalizeUrl(value: string, kind: 'login' | 'instance'): string {
        let normalized = value.trim()
        if (!/^https?:\/\//i.test(normalized)) normalized = `https://${normalized}`
        const parsed = new URL(normalized)
        const host = parsed.hostname.toLowerCase().replace(/\.$/, '')
        const isLoginHost =
            host === 'login.salesforce.com' ||
            host === 'test.salesforce.com' ||
            host.endsWith('.my.salesforce.com') ||
            host.endsWith('.sandbox.my.salesforce.com')
        const isInstanceHost = host.endsWith('.salesforce.com') || host.endsWith('.force.com')
        if (
            parsed.protocol !== 'https:' ||
            parsed.username ||
            parsed.password ||
            parsed.port ||
            parsed.search ||
            parsed.hash ||
            (parsed.pathname !== '/' && parsed.pathname !== '') ||
            (kind === 'login' ? !isLoginHost : !isInstanceHost)
        ) {
            throw new Error(`Invalid Salesforce ${kind} URL`)
        }
        return parsed.origin
    }

    async function handleSubmit() {
        isSubmitting = true
        try {
            const normalizedInstance = instanceUrl.trim()
                ? normalizeUrl(instanceUrl, 'instance')
                : ''
            const normalizedLogin = normalizeUrl(loginUrl, 'login')
            const sourceConfig: Record<string, string> = {
                login_url: normalizedLogin,
            }
            if (normalizedInstance) {
                sourceConfig.instance_url = normalizedInstance
            }

            let credentials: Record<string, string>
            let authType: AuthType

            if (authMode === 'jwt') {
                if (!normalizedInstance) {
                    throw new Error('Instance URL is required to bind Salesforce OAuth to this org')
                }
                if (!consumerKey.trim() || !privateKey.trim() || !username.trim()) {
                    throw new Error(
                        'Consumer Key, Private Key and Username are required for Connected App auth',
                    )
                }
                authType = AuthType.JWT
                credentials = {
                    client_id: consumerKey.trim(),
                    private_key: privateKey,
                    username: username.trim(),
                    login_url: normalizedLogin,
                }
                if (normalizedInstance) {
                    credentials.instance_url = normalizedInstance
                }
            } else {
                if (!instanceUrl.trim() || !accessToken.trim()) {
                    throw new Error('Instance URL and Access Token are required')
                }
                authType = AuthType.BEARER_TOKEN
                credentials = {
                    access_token: accessToken,
                    instance_url: normalizedInstance,
                }
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'Salesforce',
                    sourceType: 'salesforce',
                    config: { ...sourceConfig, instance_url: normalizedInstance },
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create Salesforce source')
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'salesforce',
                    authType,
                    credentials,
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create Salesforce service credentials')
            }

            toast.success('Salesforce connected successfully!')

            consumerKey = ''
            privateKey = ''
            username = ''
            loginUrl = 'https://login.salesforce.com'
            instanceUrl = ''
            accessToken = ''

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: unknown) {
            console.error('Error setting up Salesforce:', error)
            toast.error(error instanceof Error ? error.message : 'Failed to set up Salesforce')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        consumerKey = ''
        privateKey = ''
        username = ''
        loginUrl = 'https://login.salesforce.com'
        instanceUrl = ''
        accessToken = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Salesforce</Dialog.Title>
            <Dialog.Description>
                Set up your Salesforce integration with an External Client App (recommended) or a
                static access token for quick trials.
            </Dialog.Description>
        </Dialog.Header>

        <Tabs.Root bind:value={authMode}>
            <Tabs.List class="mt-1 mb-4 grid w-full grid-cols-2">
                <Tabs.Trigger
                    value="jwt"
                    class="data-[state=active]:bg-background data-[state=active]:text-foreground cursor-pointer data-[state=active]:shadow-sm">
                    External Client App (JWT)
                </Tabs.Trigger>
                <Tabs.Trigger
                    value="token"
                    class="data-[state=active]:bg-background data-[state=active]:text-foreground cursor-pointer data-[state=active]:shadow-sm">
                    Access Token
                </Tabs.Trigger>
            </Tabs.List>

            <Tabs.Content value="jwt" class="space-y-4 pt-1">
                <div class="space-y-2">
                    <Label for="consumer-key">Consumer Key</Label>
                    <Input
                        id="consumer-key"
                        bind:value={consumerKey}
                        placeholder="3MVG9..."
                        required />
                    <p class="text-muted-foreground text-sm">
                        The External Client App's client ID (Setup &gt; External Client App Manager
                        &gt; your app). Configure the JWT bearer flow on the app.
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="private-key">Private Key (PEM)</Label>
                    <Textarea
                        id="private-key"
                        bind:value={privateKey}
                        placeholder="-----BEGIN PRIVATE KEY-----..."
                        rows={5}
                        required />
                    <p class="text-muted-foreground text-sm">
                        The RSA private key paired with the certificate uploaded to the External
                        Client App. The connector signs its own tokens — no password or security
                        token needed.
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="username">Username</Label>
                    <Input
                        id="username"
                        bind:value={username}
                        placeholder="you@yourorg.com"
                        required />
                    <p class="text-muted-foreground text-sm">
                        The Salesforce user the app acts as.
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="login-url">Login URL</Label>
                    <Input id="login-url" bind:value={loginUrl} required />
                    <p class="text-muted-foreground text-sm">
                        https://login.salesforce.com (production) or https://test.salesforce.com
                        (sandbox).
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="instance-url">Instance URL</Label>
                    <Input
                        id="instance-url"
                        bind:value={instanceUrl}
                        placeholder="https://yourorg-dev-ed.my.salesforce.com" />
                    <p class="text-muted-foreground text-sm">
                        Required to bind this Salesforce source to the correct organization.
                    </p>
                </div>
            </Tabs.Content>

            <Tabs.Content value="token" class="space-y-4 pt-1">
                <div class="space-y-2">
                    <Label for="instance-url-token">Instance URL</Label>
                    <Input
                        id="instance-url-token"
                        bind:value={instanceUrl}
                        placeholder="https://yourorg.salesforce.com"
                        required />
                    <p class="text-muted-foreground text-sm">
                        Native-action trial only; MCP requires each user to authorize Salesforce
                        through Omni. Example: https://yourorg.salesforce.com
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="access-token">Access Token</Label>
                    <Input
                        id="access-token"
                        bind:value={accessToken}
                        placeholder="Your Salesforce access token"
                        type="password"
                        required />
                    <p class="text-muted-foreground text-sm">
                        A session id or OAuth access token. Expires after a couple of hours — this
                        mode is for quick trials only.
                    </p>
                </div>
            </Tabs.Content>
        </Tabs.Root>

        <p class="text-muted-foreground text-xs">
            Per-user MCP actions require an External Client App; configure it after connecting under
            Integrations &gt; OAuth Apps (Scoped to this org).
        </p>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                {isSubmitting ? 'Connecting...' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
