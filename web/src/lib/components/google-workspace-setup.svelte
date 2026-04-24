<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { AuthType } from '$lib/types'
    import { toast } from 'svelte-sonner'
    import { goto } from '$app/navigation'
    import { invalidateAll } from '$app/navigation'
    import googleDriveLogo from '$lib/images/icons/google-drive.svg'
    import gmailLogo from '$lib/images/icons/gmail.svg'
    import GoogleServiceAccountForm from '$lib/components/google-service-account-form.svelte'

    interface Props {
        open: boolean
        googleOAuthConfigured?: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, googleOAuthConfigured = false, onSuccess, onCancel }: Props = $props()

    let activeTab: 'service-account' | 'oauth' = $state('service-account')

    // Service Account form state
    let serviceAccountJson = $state('')
    let principalEmail = $state('')
    let domain = $state('')
    let connectDrive = $state(true)
    let connectGmail = $state(true)
    let isSubmitting = $state(false)

    // OAuth form state
    let googleOAuthClientId = $state('')
    let googleOAuthClientSecret = $state('')
    let isSavingOAuth = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!connectDrive && !connectGmail) {
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
            const config = {
                domain: domain || null,
            }
            const authType = AuthType.JWT
            const provider = 'google'

            if (connectDrive) {
                const driveSourceResponse = await fetch('/api/sources', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        name: 'Google Drive',
                        sourceType: 'google_drive',
                        config,
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
                        config,
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
                        name: 'Gmail',
                        sourceType: 'gmail',
                        config,
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
                        config,
                    }),
                })

                if (!gmailCredentialsResponse.ok) {
                    throw new Error('Failed to create Gmail service credentials')
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

    async function handleSaveOAuth() {
        if (!googleOAuthClientId || !googleOAuthClientSecret) {
            toast.error('Please enter both Client ID and Client Secret')
            return
        }

        isSavingOAuth = true
        try {
            const response = await fetch('/api/connector-configs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    provider: 'google',
                    config: {
                        oauth_client_id: googleOAuthClientId,
                        oauth_client_secret: googleOAuthClientSecret,
                    },
                }),
            })

            if (!response.ok) {
                throw new Error('Failed to save Google OAuth configuration')
            }

            toast.success('Google OAuth configuration saved')
            googleOAuthClientId = ''
            googleOAuthClientSecret = ''
            await invalidateAll()
        } catch (error: any) {
            console.error('Error saving Google OAuth config:', error)
            toast.error(error.message || 'Failed to save configuration')
        } finally {
            isSavingOAuth = false
        }
    }

    function handleCancel() {
        serviceAccountJson = ''
        principalEmail = ''
        domain = ''
        connectDrive = true
        connectGmail = true
        googleOAuthClientId = ''
        googleOAuthClientSecret = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Google Workspace</Dialog.Title>
            <Dialog.Description>Choose how to connect Google Workspace to Omni.</Dialog.Description>
        </Dialog.Header>

        <!-- Tabs -->
        <div class="border-b">
            <div class="flex gap-4">
                <button
                    class="relative cursor-pointer border-b-2 px-1 pb-2 text-sm font-medium transition-colors {activeTab ===
                    'service-account'
                        ? 'border-primary text-foreground'
                        : 'text-muted-foreground hover:text-foreground border-transparent'}"
                    onclick={() => (activeTab = 'service-account')}>
                    Service Account
                </button>
                <button
                    class="relative cursor-pointer border-b-2 px-1 pb-2 text-sm font-medium transition-colors {activeTab ===
                    'oauth'
                        ? 'border-primary text-foreground'
                        : 'text-muted-foreground hover:text-foreground border-transparent'}"
                    onclick={() => (activeTab = 'oauth')}>
                    OAuth
                    {#if googleOAuthConfigured}
                        <span
                            class="ml-1.5 inline-flex items-center rounded-full bg-green-100 px-1.5 py-0.5 text-[10px] font-medium text-green-800 dark:bg-green-900/20 dark:text-green-400">
                            Configured
                        </span>
                    {/if}
                </button>
            </div>
        </div>

        <!-- Service Account Tab -->
        {#if activeTab === 'service-account'}
            <div class="space-y-4">
                <div class="space-y-2">
                    <Label>Services to connect</Label>
                    <div class="flex gap-4">
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
                    </div>
                </div>

                <GoogleServiceAccountForm bind:serviceAccountJson bind:principalEmail bind:domain />
            </div>

            <Dialog.Footer>
                <Button variant="outline" onclick={handleCancel} class="cursor-pointer"
                    >Cancel</Button>
                <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                    {isSubmitting ? 'Connecting...' : 'Connect'}
                </Button>
            </Dialog.Footer>
        {/if}

        <!-- OAuth Tab -->
        {#if activeTab === 'oauth'}
            <div class="space-y-4">
                <p class="text-muted-foreground text-sm">
                    Configure Google OAuth credentials so that each user can individually connect
                    their Google account from their settings.
                </p>

                <div class="space-y-2">
                    <Label for="oauth-client-id">Client ID</Label>
                    <Input
                        id="oauth-client-id"
                        bind:value={googleOAuthClientId}
                        placeholder="Enter Google OAuth Client ID" />
                </div>

                <div class="space-y-2">
                    <Label for="oauth-client-secret">Client Secret</Label>
                    <Input
                        id="oauth-client-secret"
                        type="password"
                        bind:value={googleOAuthClientSecret}
                        placeholder={googleOAuthConfigured
                            ? 'Enter new secret to update'
                            : 'Enter Google OAuth Client Secret'} />
                </div>
            </div>

            <Dialog.Footer>
                <Button variant="outline" onclick={handleCancel} class="cursor-pointer"
                    >Cancel</Button>
                <Button onclick={handleSaveOAuth} disabled={isSavingOAuth} class="cursor-pointer">
                    {isSavingOAuth ? 'Saving...' : 'Save Configuration'}
                </Button>
            </Dialog.Footer>
        {/if}
    </Dialog.Content>
</Dialog.Root>
