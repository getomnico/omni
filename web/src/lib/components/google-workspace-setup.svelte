<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import * as Card from '$lib/components/ui/card'
    import { AuthType } from '$lib/types'
    import type { FolderPathFilter } from '$lib/types'
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

    // Service Account form state
    let serviceAccountJson = $state('')
    let principalEmail = $state('')
    let domain = $state('')
    let connectDrive = $state(true)
    let connectGmail = $state(true)
    let connectChat = $state(false)
    let isSubmitting = $state(false)
    let driveFolderFilters = $state<FolderPathFilter[]>([])

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

    async function handleSubmit() {
        isSubmitting = true
        try {
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

            // Validate JSON
            try {
                JSON.parse(serviceAccountJson)
            } catch {
                throw new Error('Invalid JSON format')
            }

            const credentials = { service_account_key: serviceAccountJson }

            // Base config shared by all Google services (domain only).
            const baseConfig: Record<string, unknown> = {
                domain: domain || null,
            }

            // Drive-specific config with optional folder filters.
            const driveConfig: Record<string, unknown> = {
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

            // Reset form
            serviceAccountJson = ''
            principalEmail = ''
            domain = ''

            // Call success callback if provided
            if (onSuccess) {
                onSuccess()
            } else {
                // Default behavior: redirect to integrations page
                await goto('/admin/settings/integrations')
            }
        } catch (error: any) {
            console.error('Error setting up Google Workspace:', error)
            toast.error(error.message || 'Failed to set up Google Workspace')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        serviceAccountJson = ''
        principalEmail = ''
        domain = ''
        connectDrive = true
        connectGmail = true
        connectChat = false
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
                Set up org-wide Google Drive, Gmail, and Google Chat sync using a Google service
                account with domain-wide delegation.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
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

            <GoogleServiceAccountForm bind:serviceAccountJson bind:principalEmail bind:domain />

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
                            {domain} />
                    </Card.Content>
                </Card.Root>
            {/if}
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                {isSubmitting ? 'Connecting...' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
