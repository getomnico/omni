<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Select from '$lib/components/ui/select'
    import { AuthType, type ConnectorListEntry } from '$lib/types'
    import { toast } from 'svelte-sonner'

    /** Fetch all Darwinbox action names from the registered connector manifest.
     * Groups come from extra_schema.action_groups derived from the connector's
     * internal action policy table. */
    async function getAllowedActions(readOnly: boolean): Promise<string[]> {
        const resp = await fetch('/api/connectors')
        if (!resp.ok) throw new Error('Failed to load connector actions')
        const connectors: ConnectorListEntry[] = await resp.json()
        const darwinbox = connectors.find((c) => c.source_type === 'darwinbox')
        if (!darwinbox?.manifest) throw new Error('Darwinbox connector not registered')
        const groups = darwinbox.manifest.extra_schema?.action_groups ?? {}
        const result: string[] = []
        for (const group of Object.values(groups)) {
            if (!group) continue
            for (const action of group.read ?? []) {
                if (!result.includes(action)) result.push(action)
            }
            if (readOnly) continue
            for (const action of group.write ?? []) {
                if (!result.includes(action)) result.push(action)
            }
        }
        return result
    }

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
    let readOnly = $state(true)
    let writeAcknowledged = $state(false)
    let participantEmails = $state('')
    let targetEmployeeIds = $state('')
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
            const participants = participantEmails
                .split(',')
                .map((v) => v.trim().toLowerCase())
                .filter(Boolean)
            const targets = targetEmployeeIds
                .split(',')
                .map((v) => v.trim())
                .filter(Boolean)
            if (participants.length === 0) {
                throw new Error('At least one approved participant email is required')
            }
            if (!readOnly && !writeAcknowledged) {
                throw new Error('Confirm write-mode acknowledgement before continuing')
            }
            // Derive allowed actions from registered connector manifest
            const allowedActions = await getAllowedActions(readOnly)

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'Darwinbox',
                    sourceType: 'darwinbox',
                    isActive: true,
                    config: {
                        base_url: baseUrl.trim().replace(/\/$/, ''),
                        read_only: readOnly,
                        employee_scope: {
                            mode: 'include',
                            employee_ids: targets,
                        },
                        employee_fields: [
                            'name',
                            'employee_id',
                            'company_email',
                            'department',
                            'designation',
                            'office_location',
                        ],
                        sync_modules: {
                            employee_directory: true,
                            deleted_employees: true,
                            departments: false,
                            designations: false,
                            office_locations: false,
                            business_units: false,
                            divisions: false,
                            cost_centers: false,
                            group_companies: false,
                            positions: false,
                            holidays: false,
                            ats_jobs: false,
                        },
                        action_modules: {
                            employee_self_service: true,
                            manager_workflows: true,
                            hr_operations: false,
                            ats: false,
                            reports: true,
                        },
                        authorization: {
                            actions_enabled: true,
                            write_acknowledged: writeAcknowledged,
                            participant_emails: participants,
                            allowed_actions: allowedActions,
                            target_employee_ids: targets,
                            target_employee_emails: [],
                            target_departments: [],
                            hr_admin_emails: [],
                            recruiter_emails: [],
                            allowed_report_ids: [],
                            max_batch_size: 1,
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
                throw new Error('Failed to save Darwinbox credentials')
            }

            toast.success('Darwinbox source created')
            onSuccess?.()
        } catch (e) {
            toast.error(e instanceof Error ? e.message : 'Failed to create Darwinbox source')
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
        readOnly = true
        writeAcknowledged = false
        participantEmails = ''
        targetEmployeeIds = ''
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

            <div class="space-y-3 rounded-md border p-3 text-sm">
                <div class="font-medium">Access controls</div>
                <label class="flex cursor-pointer items-center gap-2">
                    <input type="checkbox" bind:checked={readOnly} />
                    Read-only mode (prevents all mutations)
                </label>
                <div class="space-y-1">
                    <Label for="darwinbox-participants">Approved participant emails</Label>
                    <Input
                        id="darwinbox-participants"
                        bind:value={participantEmails}
                        placeholder="user1@example.com, user2@example.com" />
                </div>
                <div class="space-y-1">
                    <Label for="darwinbox-targets">Approved target employee IDs</Label>
                    <Input
                        id="darwinbox-targets"
                        bind:value={targetEmployeeIds}
                        placeholder="EMP001, EMP002" />
                </div>
                {#if !readOnly}
                    <label
                        class="flex cursor-pointer items-start gap-2 rounded border border-amber-300 p-2">
                        <input type="checkbox" bind:checked={writeAcknowledged} />
                        <span
                            >I understand that approved actions can change production Darwinbox data
                            and require explicit confirmation.</span>
                    </label>
                {/if}
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
