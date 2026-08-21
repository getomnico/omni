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
    import type { ConnectorActionResponse } from '$lib/types'
    import { toast } from 'svelte-sonner'
    import { goto } from '$app/navigation'
    import googleDriveLogo from '$lib/images/icons/google-drive.svg'
    import gmailLogo from '$lib/images/icons/gmail.svg'
    import googleChatLogo from '$lib/images/icons/google-chat.svg'
    import GoogleServiceAccountForm from '$lib/components/google-service-account-form.svelte'
    import GoogleDriveFolderSelector from '$lib/components/google-drive-folder-selector.svelte'
    import type { FolderPathFilter } from '$lib/components/google-drive-folder-selector.types'
    import { GOOGLE_SA_DIRECT_SCOPES } from '$lib/utils/google-scopes'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    type GoogleAuthMode = 'domain_wide_delegation' | 'service_account_direct'

    interface GoogleSourceConfig {
        auth_mode: GoogleAuthMode
        domain?: string | null
        folder_path_filters?: FolderPathFilter[]
    }

    interface SharedDriveAccessResult {
        drive_id: string
        ok: boolean
        role: string | null
        error: string | null
    }

    interface SharedDriveAccessResponse {
        drives: SharedDriveAccessResult[]
    }

    interface SaDirectGroupAccessResponse {
        domain: string
        groups: number
        memberships: number
    }

    interface GroupAccessStatus {
        pending: boolean
        ok: boolean
        groups: number | null
        memberships: number | null
        error: string | null
    }

    interface DriveAccessStatus {
        pending: boolean
        ok: boolean
        role: string | null
        error: string | null
    }

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
    let driveSelectorRevision = $state(0)
    let driveFiltersBeforeCredentialReplacement = $state<FolderPathFilter[] | null>(null)

    // SA-direct form state (kept separate so switching tabs preserves each)
    let saServiceAccountJson = $state('')
    let saDomain = $state('')
    let saDriveFilters = $state<FolderPathFilter[]>([])
    let saSelectorRevision = $state(0)
    let saFiltersBeforeCredentialReplacement = $state<FolderPathFilter[] | null>(null)
    // Per-drive validation: drive_id -> { pending, ok, role, error }
    let accessValidation = $state<Record<string, DriveAccessStatus>>({})
    let isValidating = $state(false)
    let validationAbort: AbortController | null = null
    let validationGeneration = $state(0)
    let groupAccessValidation = $state<GroupAccessStatus>({
        pending: false,
        ok: false,
        groups: null,
        memberships: null,
        error: null,
    })
    let groupValidationAbort: AbortController | null = null
    let groupValidationGeneration = $state(0)
    let groupValidationTimer: ReturnType<typeof setTimeout> | undefined

    // Switch to SA-direct tab: force Drive-only. The tab value itself is
    // kept in sync by Tabs.Root's bind:value, so this only applies side
    // effects.
    function selectTab(tab: GoogleAuthMode) {
        console.log('[google-setup] selectTab', tab)
        if (tab === 'service_account_direct') {
            connectDrive = true
            connectGmail = false
            connectChat = false
        }
    }

    function invalidateGroupAccessValidation() {
        groupValidationGeneration += 1
        groupValidationAbort?.abort()
        groupValidationAbort = null
        if (groupValidationTimer) {
            clearTimeout(groupValidationTimer)
            groupValidationTimer = undefined
        }
        groupAccessValidation = {
            pending: false,
            ok: false,
            groups: null,
            memberships: null,
            error: null,
        }
    }

    function invalidateSaValidation() {
        validationGeneration += 1
        validationAbort?.abort()
        validationAbort = null
        accessValidation = {}
        isValidating = false
        invalidateGroupAccessValidation()
        isSubmitting = false
    }

    function handleDwdCredentialsChange() {
        driveFolderFilters = []
        driveSelectorRevision += 1
    }

    function handleDwdAccountDetailsChange() {
        driveFiltersBeforeCredentialReplacement = null
        handleDwdCredentialsChange()
    }

    function handleDwdCredentialReplacementStart() {
        driveFiltersBeforeCredentialReplacement = [...driveFolderFilters]
    }

    function handleDwdCredentialReplacementCancel() {
        if (driveFiltersBeforeCredentialReplacement === null) return
        driveFolderFilters = driveFiltersBeforeCredentialReplacement
        driveFiltersBeforeCredentialReplacement = null
        driveSelectorRevision += 1
    }

    function handleSaCredentialsChange() {
        saDriveFilters = []
        saSelectorRevision += 1
        invalidateSaValidation()
    }

    function handleSaCredentialReplacementStart() {
        saFiltersBeforeCredentialReplacement = [...saDriveFilters]
    }

    function handleSaCredentialReplacementCancel() {
        if (saFiltersBeforeCredentialReplacement === null) return
        saDriveFilters = saFiltersBeforeCredentialReplacement
        saFiltersBeforeCredentialReplacement = null
        saSelectorRevision += 1
        invalidateSaValidation()
    }

    function handleSaAccountDetailsChange() {
        scheduleGroupAccessValidation()
    }

    // Event-driven SA-direct access validation. The drive selector reports
    // every selection change (add/remove) via onSelectedChange; validation
    // runs only when both a key and drives are present.
    function handleSaSelectionChange(filters: FolderPathFilter[]) {
        console.log(
            '[google-setup] saSelectionChange',
            filters.map((f) => f.driveId),
        )
        if (saServiceAccountJson.trim() && filters.length > 0) {
            validateAccess(saServiceAccountJson, filters)
        } else {
            invalidateSaValidation()
        }
        scheduleGroupAccessValidation()
    }

    function scheduleGroupAccessValidation() {
        invalidateGroupAccessValidation()

        if (!saServiceAccountJson.trim() || !saDomain.trim()) {
            return
        }

        groupValidationTimer = setTimeout(() => {
            groupValidationTimer = undefined
            validateGroupAccess(saServiceAccountJson, saDomain)
        }, 300)
    }

    async function validateGroupAccess(saJson: string, workspaceDomain: string) {
        const generation = ++groupValidationGeneration
        groupValidationAbort?.abort()
        const controller = new AbortController()
        groupValidationAbort = controller
        groupAccessValidation = {
            pending: true,
            ok: false,
            groups: null,
            memberships: null,
            error: null,
        }

        try {
            const response = await fetch('/api/connectors/google_drive/action', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    action: 'validate_sa_direct_group_access',
                    params: { auth_mode: 'service_account_direct' },
                    serviceAccountJson: saJson,
                    domain: workspaceDomain.trim(),
                    authMode: 'service_account_direct',
                }),
                signal: controller.signal,
            })
            if (generation !== groupValidationGeneration) return

            if (!response.ok) {
                const errBody = (await response.json().catch(() => null)) as {
                    error?: string
                    message?: string
                } | null
                groupAccessValidation = {
                    pending: false,
                    ok: false,
                    groups: null,
                    memberships: null,
                    error:
                        errBody?.error ||
                        errBody?.message ||
                        'Failed to validate Workspace group access',
                }
                return
            }

            const body =
                (await response.json()) as ConnectorActionResponse<SaDirectGroupAccessResponse>
            if (body.status === 'success' && body.result) {
                groupAccessValidation = {
                    pending: false,
                    ok: true,
                    groups: body.result.groups,
                    memberships: body.result.memberships,
                    error: null,
                }
            } else {
                groupAccessValidation = {
                    pending: false,
                    ok: false,
                    groups: null,
                    memberships: null,
                    error: body.error || 'Workspace group access could not be verified',
                }
            }
        } catch (err) {
            if (generation !== groupValidationGeneration) return
            if (err instanceof DOMException && err.name === 'AbortError') return
            groupAccessValidation = {
                pending: false,
                ok: false,
                groups: null,
                memberships: null,
                error: 'Network error while validating Workspace group access',
            }
        } finally {
            if (generation === groupValidationGeneration) {
                groupValidationAbort = null
            }
        }
    }

    async function validateAccess(saJson: string, filters: FolderPathFilter[]) {
        console.log('[google-setup] validateAccess start', filters.length)
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

    const groupAccessValidated = $derived(
        saServiceAccountJson.trim().length > 0 &&
            saDomain.trim().length > 0 &&
            !groupAccessValidation.pending &&
            groupAccessValidation.ok,
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
        if (!saDomain.trim()) {
            throw new Error('Organization domain is required for group membership sync')
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
        if (!groupAccessValidated) {
            throw new Error(
                groupAccessValidation.error ||
                    'Wait for Workspace group membership access validation to pass',
            )
        }

        const driveConfig: GoogleSourceConfig = {
            auth_mode: 'service_account_direct',
            domain: saDomain.trim(),
            folder_path_filters: saDriveFilters,
        }

        // Create ONLY a Google Drive source.
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
            const errBody = (await driveSourceResponse.json().catch(() => null)) as {
                message?: string
            } | null
            throw new Error(errBody?.message || 'Failed to create Google Drive source')
        }
        const driveSource = await driveSourceResponse.json()

        const credConfig: Record<string, unknown> = {
            domain: saDomain.trim(),
            scopes: GOOGLE_SA_DIRECT_SCOPES,
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
        driveSelectorRevision += 1
        driveFiltersBeforeCredentialReplacement = null
        saServiceAccountJson = ''
        saDomain = ''
        saDriveFilters = []
        saSelectorRevision += 1
        saFiltersBeforeCredentialReplacement = null
        invalidateSaValidation()
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
                    mode="dwd"
                    onCredentialsChange={handleDwdCredentialsChange}
                    onAccountDetailsChange={handleDwdAccountDetailsChange}
                    onCredentialReplacementStart={handleDwdCredentialReplacementStart}
                    onCredentialReplacementCancel={handleDwdCredentialReplacementCancel} />

                {#if connectDrive}
                    <Card.Root class="border-dashed">
                        <Card.Header>
                            <Card.Title class="text-sm">Drive Folder Filters</Card.Title>
                            <Card.Description class="text-xs">
                                Optionally restrict indexing to specific shared drives and folders.
                            </Card.Description>
                        </Card.Header>
                        <Card.Content>
                            {#key driveSelectorRevision}
                                <GoogleDriveFolderSelector
                                    bind:selected={driveFolderFilters}
                                    {serviceAccountJson}
                                    {principalEmail}
                                    {domain}
                                    authMode="domain_wide_delegation" />
                            {/key}
                        </Card.Content>
                    </Card.Root>
                {/if}
            </Tabs.Content>

            <Tabs.Content value="service_account_direct" class="space-y-4 pt-4">
                <GoogleServiceAccountForm
                    bind:serviceAccountJson={saServiceAccountJson}
                    bind:domain={saDomain}
                    mode="sa-direct"
                    showDomain
                    onCredentialsChange={handleSaCredentialsChange}
                    onAccountDetailsChange={handleSaAccountDetailsChange}
                    onCredentialReplacementStart={handleSaCredentialReplacementStart}
                    onCredentialReplacementCancel={handleSaCredentialReplacementCancel} />

                {#if saServiceAccountJson.trim()}
                    {#if groupAccessValidation.pending}
                        <Alert.Root>
                            <Loader2 class="h-4 w-4 animate-spin" />
                            <Alert.Title>Checking Workspace group access</Alert.Title>
                            <Alert.Description>
                                Verifying that this service account can list groups and group
                                members.
                            </Alert.Description>
                        </Alert.Root>
                    {:else if groupAccessValidation.ok}
                        <Alert.Root>
                            <CheckCircle2 class="h-4 w-4 text-green-600" />
                            <Alert.Title>Workspace group access verified</Alert.Title>
                            <Alert.Description>
                                Found {groupAccessValidation.groups} groups and
                                {groupAccessValidation.memberships} active memberships. Group membership
                                events will be emitted during sync.
                            </Alert.Description>
                        </Alert.Root>
                    {:else}
                        <Alert.Root variant="destructive">
                            <AlertCircle class="h-4 w-4" />
                            <Alert.Title>Workspace group access required</Alert.Title>
                            <Alert.Description>
                                {groupAccessValidation.error ??
                                    'Enter your Workspace domain to validate group membership access.'}
                                The service account must be able to list Workspace groups and group members
                                before this source can be created.
                            </Alert.Description>
                        </Alert.Root>
                    {/if}
                {/if}

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
                    <Card.Content class="space-y-3">
                        {#key saSelectorRevision}
                            <GoogleDriveFolderSelector
                                bind:selected={saDriveFilters}
                                serviceAccountJson={saServiceAccountJson}
                                authMode="service_account_direct"
                                label="Shared drives to index"
                                description="Search and select shared drives to index in full. Only whole drives are supported in this mode, and at least one is required."
                                disabled={!saServiceAccountJson.trim()}
                                onSelectedChange={handleSaSelectionChange} />
                        {/key}
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
                        (!allDrivesValidated ||
                            !groupAccessValidated ||
                            validationInProgress ||
                            groupAccessValidation.pending))}
                class="cursor-pointer">
                {#if isSubmitting}
                    <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                {/if}
                {activeTab === 'service_account_direct' ? 'Connect and Sync' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
