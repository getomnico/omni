<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Select from '$lib/components/ui/select'
    import { AuthType } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    type DarwinboxAuthMode = 'basic' | 'client_credentials' | 'dynamic_token'
    type DarwinboxGrantType = 'authorization_code' | 'refresh_token'

    let baseUrl = $state('')
    let authMode = $state<DarwinboxAuthMode>('basic')
    let username = $state('')
    let password = $state('')
    let apiKey = $state('')
    let clientId = $state('')
    let clientSecret = $state('')
    let grantType = $state<DarwinboxGrantType>('refresh_token')
    let authorizationCode = $state('')
    let refreshToken = $state('')
    let datasetKey = $state('')
    let defaultTimezone = $state('Asia/Kolkata')
    let enablePositions = $state(false)
    let enableAts = $state(false)
    let enableHrActions = $state(false)
    let enableReports = $state(false)
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!baseUrl.trim()) throw new Error('Darwinbox base URL is required')
            if (authMode === 'basic') {
                if (!username.trim()) throw new Error('Username is required')
                if (!password.trim()) throw new Error('Password is required')
                if (!apiKey.trim()) throw new Error('API key is required for Basic auth')
            } else {
                if (!clientId.trim()) throw new Error('Client ID is required')
                if (!clientSecret.trim()) throw new Error('Client secret is required')
                if (
                    authMode === 'dynamic_token' &&
                    grantType === 'authorization_code' &&
                    !authorizationCode.trim()
                ) {
                    throw new Error('Authorization code is required')
                }
                if (
                    authMode === 'dynamic_token' &&
                    grantType === 'refresh_token' &&
                    !refreshToken.trim()
                ) {
                    throw new Error('Refresh token is required')
                }
            }
            if (!datasetKey.trim()) throw new Error('Dataset key is required')

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'Darwinbox',
                    sourceType: 'darwinbox',
                    config: {
                        base_url: baseUrl.trim().replace(/\/$/, ''),
                        default_timezone: defaultTimezone.trim() || null,
                        sync_modules: {
                            employee_directory: true,
                            deleted_employees: true,
                            org_masters: true,
                            positions: enablePositions,
                            holidays: true,
                            ats_jobs: enableAts,
                        },
                        action_modules: {
                            employee_self_service: true,
                            manager_workflows: true,
                            hr_operations: enableHrActions,
                            ats: enableAts,
                            reports: enableReports,
                        },
                        authorization: {
                            use_darwinbox_permissions: true,
                            hr_admin_emails: [],
                            recruiter_emails: [],
                        },
                    },
                }),
            })

            if (!sourceResponse.ok) throw new Error('Failed to create Darwinbox source')
            const source = await sourceResponse.json()

            const credentials =
                authMode === 'basic'
                    ? {
                          auth_type: 'basic',
                          username,
                          password,
                          api_key: apiKey,
                          dataset_key: datasetKey,
                      }
                    : authMode === 'client_credentials'
                      ? {
                            auth_type: 'client_credentials',
                            client_id: clientId,
                            client_secret: clientSecret,
                            api_key: apiKey.trim() || null,
                            dataset_key: datasetKey,
                        }
                      : {
                            auth_type: 'dynamic_token',
                            client_id: clientId,
                            client_secret: clientSecret,
                            grant_type: grantType,
                            code: grantType === 'authorization_code' ? authorizationCode : null,
                            refresh_token: grantType === 'refresh_token' ? refreshToken : null,
                            api_key: apiKey.trim() || null,
                            dataset_key: datasetKey,
                        }

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'darwinbox',
                    authType: authMode === 'basic' ? AuthType.BASIC_AUTH : AuthType.OAUTH,
                    credentials,
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create Darwinbox service credentials')
            }

            toast.success('Darwinbox connected successfully!')
            reset()
            onSuccess?.()
        } catch (error: any) {
            console.error('Error setting up Darwinbox:', error)
            toast.error(error.message || 'Failed to set up Darwinbox')
        } finally {
            isSubmitting = false
        }
    }

    function reset() {
        baseUrl = ''
        authMode = 'basic'
        username = ''
        password = ''
        apiKey = ''
        clientId = ''
        clientSecret = ''
        grantType = 'refresh_token'
        authorizationCode = ''
        refreshToken = ''
        datasetKey = ''
        defaultTimezone = 'Asia/Kolkata'
        enablePositions = false
        enableAts = false
        enableHrActions = false
        enableReports = false
    }

    function handleCancel() {
        reset()
        onCancel?.()
    }

    function authModeLabel(mode: DarwinboxAuthMode): string {
        switch (mode) {
            case 'basic':
                return 'Basic auth + API key'
            case 'client_credentials':
                return 'OAuth2 client credentials'
            case 'dynamic_token':
                return 'Legacy dynamic token'
        }
    }

    function grantTypeLabel(type: DarwinboxGrantType): string {
        switch (type) {
            case 'refresh_token':
                return 'Refresh token'
            case 'authorization_code':
                return 'Authorization code'
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Darwinbox</Dialog.Title>
            <Dialog.Description>
                Sync Darwinbox employee directory and organization data, and enable HR workflow
                actions for agents.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="darwinbox-url">Darwinbox base URL</Label>
                <Input
                    id="darwinbox-url"
                    bind:value={baseUrl}
                    placeholder="https://acme.darwinbox.in"
                    required />
            </div>
            <div class="space-y-2">
                <Label for="darwinbox-auth-mode">Authentication mode</Label>
                <Select.Root type="single" bind:value={authMode}>
                    <Select.Trigger id="darwinbox-auth-mode" class="w-full">
                        {authModeLabel(authMode)}
                    </Select.Trigger>
                    <Select.Content>
                        <Select.Item value="basic">Basic auth + API key</Select.Item>
                        <Select.Item value="client_credentials"
                            >OAuth2 client credentials</Select.Item>
                        <Select.Item value="dynamic_token">Legacy dynamic token</Select.Item>
                    </Select.Content>
                </Select.Root>
                <p class="text-muted-foreground text-xs">
                    Basic auth requires the Darwinbox API username, password, and API key. Token
                    modes use client credentials and only require an API key if your tenant expects
                    it.
                </p>
            </div>
            {#if authMode === 'basic'}
                <div class="grid gap-4 md:grid-cols-2">
                    <div class="space-y-2">
                        <Label for="darwinbox-username">API username</Label>
                        <Input id="darwinbox-username" bind:value={username} required />
                    </div>
                    <div class="space-y-2">
                        <Label for="darwinbox-password">API password</Label>
                        <Input
                            id="darwinbox-password"
                            bind:value={password}
                            type="password"
                            required />
                    </div>
                </div>
            {:else}
                <div class="grid gap-4 md:grid-cols-2">
                    <div class="space-y-2">
                        <Label for="darwinbox-client-id">Client ID</Label>
                        <Input id="darwinbox-client-id" bind:value={clientId} required />
                    </div>
                    <div class="space-y-2">
                        <Label for="darwinbox-client-secret">Client secret</Label>
                        <Input
                            id="darwinbox-client-secret"
                            bind:value={clientSecret}
                            type="password"
                            required />
                    </div>
                </div>
                {#if authMode === 'dynamic_token'}
                    <div class="grid gap-4 md:grid-cols-2">
                        <div class="space-y-2">
                            <Label for="darwinbox-grant-type">Grant type</Label>
                            <Select.Root type="single" bind:value={grantType}>
                                <Select.Trigger id="darwinbox-grant-type" class="w-full">
                                    {grantTypeLabel(grantType)}
                                </Select.Trigger>
                                <Select.Content>
                                    <Select.Item value="refresh_token">Refresh token</Select.Item>
                                    <Select.Item value="authorization_code"
                                        >Authorization code</Select.Item>
                                </Select.Content>
                            </Select.Root>
                        </div>
                        <div class="space-y-2">
                            {#if grantType === 'refresh_token'}
                                <Label for="darwinbox-refresh-token">Refresh token</Label>
                                <Input
                                    id="darwinbox-refresh-token"
                                    bind:value={refreshToken}
                                    type="password"
                                    required />
                            {:else}
                                <Label for="darwinbox-authorization-code">Authorization code</Label>
                                <Input
                                    id="darwinbox-authorization-code"
                                    bind:value={authorizationCode}
                                    type="password"
                                    required />
                            {/if}
                        </div>
                    </div>
                {/if}
            {/if}
            <div class="grid gap-4 md:grid-cols-2">
                <div class="space-y-2">
                    <Label for="darwinbox-api-key">
                        API key{authMode === 'basic' ? '' : ' (optional)'}
                    </Label>
                    <Input id="darwinbox-api-key" bind:value={apiKey} type="password" />
                </div>
                <div class="space-y-2">
                    <Label for="darwinbox-dataset-key">Dataset key</Label>
                    <Input
                        id="darwinbox-dataset-key"
                        bind:value={datasetKey}
                        type="password"
                        required />
                </div>
            </div>
            <div class="space-y-2">
                <Label for="darwinbox-timezone">Default timezone</Label>
                <Input
                    id="darwinbox-timezone"
                    bind:value={defaultTimezone}
                    placeholder="Asia/Kolkata" />
            </div>
            <div class="space-y-2 rounded-md border p-3 text-sm">
                <div class="font-medium">Optional modules</div>
                <label class="flex items-center gap-2"
                    ><input type="checkbox" bind:checked={enablePositions} /> Sync position master</label>
                <label class="flex items-center gap-2"
                    ><input type="checkbox" bind:checked={enableAts} /> Enable ATS jobs/actions</label>
                <label class="flex items-center gap-2"
                    ><input type="checkbox" bind:checked={enableHrActions} /> Enable HR lifecycle actions</label>
                <label class="flex items-center gap-2"
                    ><input type="checkbox" bind:checked={enableReports} /> Enable report actions</label>
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
