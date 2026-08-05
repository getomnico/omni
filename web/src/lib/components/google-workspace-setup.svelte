<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import * as Card from '$lib/components/ui/card'
    import * as Tabs from '$lib/components/ui/tabs'
    import * as Alert from '$lib/components/ui/alert'
    import { Loader2, CheckCircle2, XCircle, AlertCircle } from '@lucide/svelte'
    import { AuthType } from '$lib/types'
    import type {
        ConnectorActionResponse,
        DriveAccessStatus,
        FolderPathFilter,
        GoogleAuthMode,
        GoogleSourceConfig,
        SharedDriveAccessResponse,
    } from '$lib/types'
    import { toast } from 'svelte-sonner'
    import { goto } from '$app/navigation'
    import googleDriveLogo from '$lib/images/icons/google-drive.svg'
    import gmailLogo from '$lib/images/icons/gmail.svg'
    import googleChatLogo from '$lib/images/icons/google-chat.svg'
    import GoogleServiceAccountForm from '$lib/components/google-service-account-form.svelte'
    import GoogleDriveFolderSelector from '$lib/components/google-drive-folder-selector.svelte'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    // Active tab: 'dwd' (domain-wide delegation) | 'sa-direct' (shared drive, no DWD)
    let activeTab = $state<GoogleAuthMode>('domain_wide_delegation')

    // Shared DWD form state
    let serviceAccountJson = $state('')
    let principalEmail = $state('')
    let domain = $state('')
    let connectDrive = $state(true)
    let connectGmail = $state(true)
    let connectChat = $state(false)
    let isSubmitting = $state(false)
    let driveFolderFilters = $state<FolderPathFilter[]>([])

    // SA-direct form state (kept separate so switching tabs preserves each)
    let saServiceAccountJson = $state('')
    let saDriveFilters = $state<FolderPathFilter[]>([])
    // Per-drive validation: drive_id -> { pending, ok, role, error }
    let accessValidation = $state<Record<string, DriveAccessStatus>>({})
    let isValidating = $state(false)
    let validationAbort: AbortController | null = null
    let validationGeneration = $state(0)

    // Credential-context guard: clear selected folder filters whenever the
    // credential inputs change after initialization so filters from one set
    // of credentials cannot be silently submitted with another.
    let prevCredContext = $state('')
    let credInitialized = $state(false)

    $effect(() => {
        const token = JSON.stringify({
            sa: serviceAccountJson,
            pe: principalEmail,
            dm: domain,
        })
        if (!credInitialized) {
            credInitialized = true
            prevCredContext = token
            return
        }
        if (token !== prevCredContext) {
            prevCredContext = token
            driveFolderFilters = []
        }
    })

    // SA-direct: when the SA key changes, the selector clears its own
    // selection (its credential context includes the key). Re-run access
    // validation whenever the key or selected drives change; if inputs become
    // empty/invalid, invalidate any in-flight validation so stale results
    // from a previous key can't land.
    $effect(() => {
        if (activeTab !== 'service_account_direct') return
        if (!saServiceAccountJson.trim() || saDriveFilters.length === 0) {
            // Invalidate and cancel any in-flight validation so stale results
            // from a previous key can't land.
            validationGeneration += 1
            validationAbort?.abort()
            validationAbort = null
            accessValidation = {}
            isSubmitting = false
            return
        }
        validateAccess(saServiceAccountJson, saDriveFilters)
    })

    // Switch to SA-direct tab: force Drive-only. activeTab itself is kept in
    // sync by the Tabs.Root bind:value, so this only applies side effects.
    function selectTab(tab: GoogleAuthMode) {
        if (tab === 'service_account_direct') {
            connectDrive = true
            connectGmail = false
            connectChat = false
        }
    }

    async function validateAccess(saJson: string, filters: FolderPathFilter[]) {
        const generation = ++validationGeneration
        validationAbort?.abort()
        const controller = new AbortController()
        validationAbort = controller
        isValidating = true

        // Mark all drives pending.
        const pending: Record<string, DriveAccessStatus> = {}
        for (const f of filters) {
            pending[f.driveId] = { pending: true, ok: false, role: null, error: null }
        }
        accessValidation = pending

        try {
            const response = await fetch('/api/connectors/google_drive/action', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    action: 'validate_shared_drive_access',
                    params: {
                        drive_ids: filters.map((f) => f.driveId),
                    },
                    serviceAccountJson: saJson,
                    authMode: 'service_account_direct',
                }),
                signal: controller.signal,
            })
            if (generation !== validationGeneration) return
            if (!response.ok) {
                const errBody = (await response.json().catch(() => null)) as {
                    error?: string
                    message?: string
                } | null
                const msg = errBody?.error || errBody?.message || 'Failed to validate drive access'
                for (const f of filters) {
                    pending[f.driveId] = {
                        pending: false,
                        ok: false,
                        role: null,
                        error: msg,
                    }
                }
                accessValidation = { ...pending }
                return
            }
            const body =
                (await response.json()) as ConnectorActionResponse<SharedDriveAccessResponse>
            const statusOk = body?.status === 'success'
            const drives = body?.result?.drives ?? []
            if (statusOk && drives.length > 0) {
                const next: typeof pending = {}
                for (const d of drives) {
                    next[d.drive_id] = {
                        pending: false,
                        ok: Boolean(d.ok),
                        role: d.role,
                        error: d.error,
                    }
                }
                accessValidation = next
            } else {
                for (const f of filters) {
                    pending[f.driveId] = {
                        pending: false,
                        ok: false,
                        role: null,
                        error: 'Drive access could not be verified',
                    }
                }
                accessValidation = { ...pending }
            }
        } catch (err) {
            if (generation !== validationGeneration) return
            if (err instanceof DOMException && err.name === 'AbortError') return
            for (const f of filters) {
                pending[f.driveId] = {
                    pending: false,
                    ok: false,
                    role: null,
                    error: 'Network error while validating drive access',
                }
            }
            accessValidation = { ...pending }
        } finally {
            if (generation === validationGeneration) {
                isValidating = false
            }
        }
    }

    const allDrivesValidated = $derived(
        saDriveFilters.length > 0 &&
            saDriveFilters.every((f) => {
                const v = accessValidation[f.driveId]
                return v && !v.pending && v.ok
            }),
    )

    const validationInProgress = $derived(
        saDriveFilters.some((f) => accessValidation[f.driveId]?.pending),
    )

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (activeTab === 'service_account_direct') {
                await handleSaDirectSubmit()
                return
            }
            await handleDwdSubmit()
        } catch (error: any) {
            console.error('Error setting up Google Workspace:', error)
            toast.error(error.message || 'Failed to set up Google Workspace')
        } finally {
            isSubmitting = false
        }
    }

    async function handleSaDirectSubmit() {
        // Defensive: never create Gmail/Chat sources in SA-direct mode.
        if (connectGmail || connectChat) {
            throw new Error('Shared drive (no DWD) setup is Drive-only')
        }
        if (!saServiceAccountJson.trim()) {
            throw new Error('Service account JSON is required')
        }
        try {
            JSON.parse(saServiceAccountJson)
        } catch {
            throw new Error('Invalid JSON format')
        }
        if (saDriveFilters.length === 0) {
            throw new Error('Select at least one shared drive to index')
        }
        if (!allDrivesValidated) {
            throw new Error(
                'Wait for every selected shared drive to pass access validation (Content manager or Manager required)',
            )
        }

        const driveConfig: GoogleSourceConfig = {
            auth_mode: 'service_account_direct',
            folder_path_filters: saDriveFilters,
        }

        // Create ONLY a Google Drive source.
        const driveSourceResponse = await fetch('/api/sources', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                scope: 'org',
                name: 'Google Drive — Shared Drive Sync',
                sourceType: 'google_drive',
                config: driveConfig,
            }),
        })
        if (!driveSourceResponse.ok) {
            const errBody = (await driveSourceResponse.json().catch(() => null)) as {
                message?: string
            } | null
            throw new Error(errBody?.message || 'Failed to create Google Drive source')
        }
        const driveSource = await driveSourceResponse.json()

        const credConfig: Record<string, unknown> = {
            scopes: ['https://www.googleapis.com/auth/drive.readonly'],
        }
        const credentialsResponse = await fetch('/api/service-credentials', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                sourceId: driveSource.id,
                provider: 'google',
                authType: AuthType.JWT,
                principalEmail: null,
                credentials: { service_account_key: saServiceAccountJson },
                config: credConfig,
            }),
        })
        if (!credentialsResponse.ok) {
            throw new Error('Failed to create Google Drive service credentials')
        }

        toast.success('Shared drive connected successfully!')
        resetForm()
        if (onSuccess) {
            onSuccess()
        } else {
            await goto('/admin/settings/integrations')
        }
    }

    async function handleDwdSubmit() {
        if (!connectDrive && !connectGmail && !connectChat) {
            throw new Error('Please select at least one service to connect')
        }

        if (!serviceAccountJson.trim()) {
            throw new Error('Service account JSON is required')
        }

        if (!principalEmail.trim()) {
            throw new Error('Admin email is required')
        }

        if (!domain.trim()) {
            throw new Error('Organization domain is required')
        }

        try {
            JSON.parse(serviceAccountJson)
        } catch {
            throw new Error('Invalid JSON format')
        }

        const credentials = { service_account_key: serviceAccountJson }

        // Base config shared by all Google services (domain only).
        const baseConfig: GoogleSourceConfig = {
            auth_mode: 'domain_wide_delegation',
            domain: domain || null,
        }

        // Drive-specific config with optional folder filters.
        const driveConfig: GoogleSourceConfig = {
            auth_mode: 'domain_wide_delegation',
            domain: domain || null,
        }
        if (driveFolderFilters.length > 0) {
            driveConfig.folder_path_filters = driveFolderFilters
        }

        const authType = AuthType.JWT
        const provider = 'google'

        if (connectDrive) {
            const driveSourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'Google Drive',
                    sourceType: 'google_drive',
                    config: driveConfig,
                }),
            })

            if (!driveSourceResponse.ok) {
                throw new Error('Failed to create Google Drive source')
            }

            const driveSource = await driveSourceResponse.json()

            const driveCredentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: driveSource.id,
                    provider: provider,
                    authType: authType,
                    principalEmail: principalEmail || null,
                    credentials,
                    config: driveConfig,
                }),
            })

            if (!driveCredentialsResponse.ok) {
                throw new Error('Failed to create Google Drive service credentials')
            }
        }

        if (connectGmail) {
            const gmailSourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'Gmail',
                    sourceType: 'gmail',
                    config: baseConfig,
                }),
            })

            if (!gmailSourceResponse.ok) {
                throw new Error('Failed to create Gmail source')
            }

            const gmailSource = await gmailSourceResponse.json()

            const gmailCredentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: gmailSource.id,
                    provider: provider,
                    authType: authType,
                    principalEmail: principalEmail || null,
                    credentials: credentials,
                    config: baseConfig,
                }),
            })

            if (!gmailCredentialsResponse.ok) {
                throw new Error('Failed to create Gmail service credentials')
            }
        }

        if (connectChat) {
            const chatSourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'Google Chat',
                    sourceType: 'google_chat',
                    config: baseConfig,
                }),
            })

            if (!chatSourceResponse.ok) {
                throw new Error('Failed to create Google Chat source')
            }

            const chatSource = await chatSourceResponse.json()

            const chatCredentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: chatSource.id,
                    provider: provider,
                    authType: authType,
                    principalEmail: principalEmail || null,
                    credentials: credentials,
                    config: baseConfig,
                }),
            })

            if (!chatCredentialsResponse.ok) {
                throw new Error('Failed to create Google Chat service credentials')
            }
        }

        toast.success('Google Workspace connected successfully!')
        resetForm()
        if (onSuccess) {
            onSuccess()
        } else {
            await goto('/admin/settings/integrations')
        }
    }

    function resetForm() {
        serviceAccountJson = ''
        principalEmail = ''
        domain = ''
        connectDrive = true
        connectGmail = true
        connectChat = false
        driveFolderFilters = []
        saServiceAccountJson = ''
        saDriveFilters = []
        accessValidation = {}
        activeTab = 'domain_wide_delegation'
    }

    function handleCancel() {
        resetForm()
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Google Workspace</Dialog.Title>
            <Dialog.Description>
                Set up Google Drive sync with a service account — either org-wide with domain-wide
                delegation, or for one or more shared drives without DWD.
            </Dialog.Description>
        </Dialog.Header>

        <Tabs.Root bind:value={activeTab} onValueChange={(v) => selectTab(v as GoogleAuthMode)}>
            <Tabs.List class="grid w-full grid-cols-2">
                <Tabs.Trigger
                    value="domain_wide_delegation"
                    class="data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm">
                    Domain-wide delegation
                </Tabs.Trigger>
                <Tabs.Trigger
                    value="service_account_direct"
                    class="data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm">
                    Shared drive (no DWD)
                </Tabs.Trigger>
            </Tabs.List>

            <Tabs.Content value="domain_wide_delegation" class="space-y-4 pt-4">
                <div class="space-y-2">
                    <Label>Services to connect</Label>
                    <div class="grid gap-4 sm:grid-cols-2">
                        <label
                            class="hover:bg-muted/50 flex flex-1 cursor-pointer items-center gap-3 rounded-lg border p-3">
                            <Checkbox bind:checked={connectDrive} />
                            <img src={googleDriveLogo} alt="Google Drive" class="h-5 w-5" />
                            <span class="font-medium">Google Drive</span>
                        </label>
                        <label
                            class="hover:bg-muted/50 flex flex-1 cursor-pointer items-center gap-3 rounded-lg border p-3">
                            <Checkbox bind:checked={connectGmail} />
                            <img src={gmailLogo} alt="Gmail" class="h-5 w-5" />
                            <span class="font-medium">Gmail</span>
                        </label>
                        <label
                            class="hover:bg-muted/50 flex flex-1 cursor-pointer items-center gap-3 rounded-lg border p-3">
                            <Checkbox bind:checked={connectChat} />
                            <img src={googleChatLogo} alt="Google Chat" class="h-5 w-5" />
                            <span class="font-medium">Google Chat</span>
                        </label>
                    </div>
                </div>

                <GoogleServiceAccountForm
                    bind:serviceAccountJson
                    bind:principalEmail
                    bind:domain
                    mode="dwd" />

                {#if connectDrive}
                    <Card.Root class="border-dashed">
                        <Card.Header>
                            <Card.Title class="text-sm">Drive Folder Filters</Card.Title>
                            <Card.Description class="text-xs">
                                Optionally restrict indexing to specific shared drives and folders.
                            </Card.Description>
                        </Card.Header>
                        <Card.Content>
                            <GoogleDriveFolderSelector
                                bind:selected={driveFolderFilters}
                                {serviceAccountJson}
                                {principalEmail}
                                {domain}
                                authMode="domain_wide_delegation" />
                        </Card.Content>
                    </Card.Root>
                {/if}
            </Tabs.Content>

            <Tabs.Content value="service_account_direct" class="space-y-4 pt-4">
                <GoogleServiceAccountForm
                    bind:serviceAccountJson={saServiceAccountJson}
                    mode="sa-direct" />

                {#if saDriveFilters.length > 0 && !allDrivesValidated}
                    <Alert.Root>
                        <AlertCircle class="h-4 w-4" />
                        <Alert.Title>Shared drive access required</Alert.Title>
                        <Alert.Description>
                            Add the service account email to each shared drive as
                            <span class="font-medium"> Content manager </span> or
                            <span class="font-medium"> Manager</span>. Viewer/Commenter roles cannot
                            read ACLs and the sync will fail closed. This setup is Drive-only —
                            Gmail and Google Chat are not available in this mode.
                        </Alert.Description>
                    </Alert.Root>
                {/if}

                <Card.Root class="border-dashed">
                    <Card.Header>
                        <Card.Title class="text-sm">Shared Drives to Index</Card.Title>
                        <Card.Description class="text-xs">
                            Select one or more shared drives to index in full. At least one is
                            required.
                        </Card.Description>
                    </Card.Header>
                    <Card.Content class="space-y-3">
                        <GoogleDriveFolderSelector
                            bind:selected={saDriveFilters}
                            serviceAccountJson={saServiceAccountJson}
                            authMode="service_account_direct"
                            label="Shared drives to index"
                            description="Search and select shared drives. Only whole drives are supported in this mode."
                            disabled={!saServiceAccountJson.trim()} />
                    </Card.Content>
                </Card.Root>

                {#if saDriveFilters.length > 0}
                    <div class="space-y-2">
                        <Label class="text-sm font-medium">Access Validation</Label>
                        <div class="space-y-2">
                            {#each saDriveFilters as filter (filter.driveId)}
                                {@const v = accessValidation[filter.driveId]}
                                <div
                                    class="flex items-center justify-between rounded-lg border p-3 text-sm">
                                    <div class="flex items-center gap-2">
                                        {#if v?.pending}
                                            <Loader2 class="h-4 w-4 animate-spin" />
                                        {:else if v?.ok}
                                            <CheckCircle2 class="h-4 w-4 text-green-600" />
                                        {:else}
                                            <XCircle class="h-4 w-4 text-red-600" />
                                        {/if}
                                        <span class="font-medium">{filter.name}</span>
                                        {#if v?.ok && v?.role}
                                            <span class="text-muted-foreground text-xs">
                                                — {v.role === 'organizer'
                                                    ? 'Manager'
                                                    : 'Content manager'}
                                            </span>
                                        {/if}
                                    </div>
                                    {#if v && !v.pending && !v.ok && v.error}
                                        <span class="text-destructive max-w-56 text-right text-xs">
                                            {v.error}
                                        </span>
                                    {/if}
                                </div>
                            {/each}
                        </div>
                        {#if validationInProgress}
                            <p class="text-muted-foreground text-xs">
                                Checking service account role on each selected drive…
                            </p>
                        {/if}
                    </div>
                {/if}
            </Tabs.Content>
        </Tabs.Root>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button
                onclick={handleSubmit}
                disabled={isSubmitting ||
                    (activeTab === 'service_account_direct' &&
                        (!allDrivesValidated || validationInProgress))}
                class="cursor-pointer">
                {#if isSubmitting}
                    <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                {/if}
                {activeTab === 'service_account_direct' ? 'Connect and Sync' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
