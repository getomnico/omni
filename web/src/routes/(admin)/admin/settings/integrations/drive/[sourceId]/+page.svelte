<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Badge } from '$lib/components/ui/badge'
    import { ArrowLeft, Search, X, AlertCircle, Info, Loader2 } from '@lucide/svelte'
    import RemoveSourceDialog from '../../remove-source-dialog.svelte'
    import GoogleServiceAccountForm from '$lib/components/google-service-account-form.svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import type {
        GoogleDirectoryUser,
        SearchUsersResponse,
        ConnectorActionResponse,
    } from '$lib/types/search'
    import { AuthType } from '$lib/types'
    import googleDriveLogo from '$lib/images/icons/google-drive.svg'

    let { data }: PageProps = $props()
    const isJwt = data.authType === AuthType.JWT
    const isOAuth = data.authType === AuthType.OAUTH

    let enabled = $state(data.source.isActive)
    let userFilterMode = $state(data.source.userFilterMode || 'all')
    let selectedUsers = $state<string[]>([])

    let serviceAccountJson = $state('')
    let principalEmail = $state(data.principalEmail)
    let domain = $state(data.domain)

    let searchQuery = $state('')
    let searchResults = $state<GoogleDirectoryUser[]>([])
    let isSearching = $state(false)
    let searchDebounceTimer: ReturnType<typeof setTimeout>

    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)
    let showRemoveDialog = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    let originalEnabled = data.source.isActive
    let originalUserFilterMode = data.source.userFilterMode || 'all'
    let originalSelectedUsers: string[] = []
    let originalPrincipalEmail = data.principalEmail
    let originalDomain = data.domain

    async function searchUsers() {
        if (searchQuery.trim().length < 2) {
            searchResults = []
            return
        }

        isSearching = true
        try {
            const response = await fetch(`/api/sources/${data.source.id}/action`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    action: 'search_users',
                    params: { q: searchQuery, limit: 20 },
                }),
            })
            if (response.ok) {
                const result =
                    (await response.json()) as ConnectorActionResponse<SearchUsersResponse>
                const statusOk = result.status === 'ok' || result.status === 'success'
                if (statusOk && result.result?.users) {
                    searchResults = result.result.users.filter(
                        (user: GoogleDirectoryUser) => !user.suspended,
                    )
                } else {
                    searchResults = []
                }
            } else {
                console.error('Failed to search users')
                searchResults = []
            }
        } catch (error) {
            console.error('Error searching users:', error)
            searchResults = []
        } finally {
            isSearching = false
        }
    }

    function handleSearchInput() {
        clearTimeout(searchDebounceTimer)
        searchDebounceTimer = setTimeout(() => {
            searchUsers()
        }, 300)
    }

    function addUser(email: string) {
        if (!selectedUsers.includes(email)) {
            selectedUsers = [...selectedUsers, email]
        }
        searchQuery = ''
        searchResults = []
    }

    function removeUser(email: string) {
        selectedUsers = selectedUsers.filter((u) => u !== email)
    }

    function validateForm() {
        formErrors = []

        if (!isJwt) {
            return true
        }

        if (enabled && userFilterMode === 'whitelist' && selectedUsers.length === 0) {
            formErrors = [...formErrors, 'Whitelist mode requires at least one user']
        }

        if (!principalEmail.trim()) {
            formErrors = [...formErrors, 'Admin email is required']
        }
        if (!domain.trim()) {
            formErrors = [...formErrors, 'Organization domain is required']
        }

        if (serviceAccountJson.trim()) {
            try {
                JSON.parse(serviceAccountJson)
            } catch {
                formErrors = [...formErrors, 'Service account JSON is not valid JSON']
            }
        }

        return formErrors.length === 0
    }

    onMount(() => {
        if (data.source.userWhitelist) {
            const whitelist =
                typeof data.source.userWhitelist === 'string'
                    ? JSON.parse(data.source.userWhitelist)
                    : data.source.userWhitelist
            if (userFilterMode === 'whitelist') {
                selectedUsers = Array.isArray(whitelist) ? whitelist : []
                originalSelectedUsers = [...selectedUsers]
            }
        }
        if (data.source.userBlacklist) {
            const blacklist =
                typeof data.source.userBlacklist === 'string'
                    ? JSON.parse(data.source.userBlacklist)
                    : data.source.userBlacklist
            if (userFilterMode === 'blacklist') {
                selectedUsers = Array.isArray(blacklist) ? blacklist : []
                originalSelectedUsers = [...selectedUsers]
            }
        }

        beforeUnloadHandler = (e: BeforeUnloadEvent) => {
            if (hasUnsavedChanges && !skipUnsavedCheck) {
                e.preventDefault()
                e.returnValue = ''
            }
        }

        window.addEventListener('beforeunload', beforeUnloadHandler)

        return () => {
            if (beforeUnloadHandler) {
                window.removeEventListener('beforeunload', beforeUnloadHandler)
            }
        }
    })

    beforeNavigate(({ cancel }) => {
        if (hasUnsavedChanges && !skipUnsavedCheck) {
            const shouldLeave = confirm(
                'You have unsaved changes. Are you sure you want to leave this page?',
            )
            if (!shouldLeave) {
                cancel()
            }
        }
    })

    $effect(() => {
        if (!isJwt) {
            hasUnsavedChanges = enabled !== originalEnabled
            return
        }

        const usersChanged =
            JSON.stringify(selectedUsers.sort()) !== JSON.stringify(originalSelectedUsers.sort())

        hasUnsavedChanges =
            enabled !== originalEnabled ||
            userFilterMode !== originalUserFilterMode ||
            usersChanged ||
            principalEmail !== originalPrincipalEmail ||
            domain !== originalDomain ||
            serviceAccountJson.trim().length > 0
    })
</script>

<svelte:head>
    <title>Configure Google Drive - {data.source.name}</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-4">
        <a
            href="/admin/settings/integrations"
            class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-sm transition-colors">
            <ArrowLeft class="h-4 w-4" />
            Back to Integrations
        </a>

        {#if formErrors.length > 0}
            <Alert.Root variant="destructive" class="mb-6">
                <AlertCircle class="h-4 w-4" />
                <Alert.Title>Configuration Error</Alert.Title>
                <Alert.Description>
                    <ul class="list-inside list-disc">
                        {#each formErrors as error}
                            <li>{error}</li>
                        {/each}
                    </ul>
                </Alert.Description>
            </Alert.Root>
        {/if}

        <form
            method="POST"
            use:enhance={() => {
                if (!validateForm()) {
                    return async () => {}
                }
                isSubmitting = true
                return async ({ result, update }) => {
                    if (result.type === 'redirect') {
                        skipUnsavedCheck = true
                        hasUnsavedChanges = false

                        if (beforeUnloadHandler) {
                            window.removeEventListener('beforeunload', beforeUnloadHandler)
                            beforeUnloadHandler = null
                        }
                    }

                    await update()
                    isSubmitting = false
                }
            }}
            class="space-y-4">
            <Card.Root class="relative">
                <Card.Header>
                    <div class="flex items-start justify-between">
                        <div>
                            <Card.Title class="flex items-center gap-2">
                                <img src={googleDriveLogo} alt="Google Drive" class="h-5 w-5" />
                                {data.source.name}
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index documents, spreadsheets, presentations, and files
                            </Card.Description>
                        </div>
                        <div class="flex items-center gap-2">
                            <Label for="enabled" class="text-sm">Enabled</Label>
                            <Switch
                                id="enabled"
                                bind:checked={enabled}
                                name="enabled"
                                class="cursor-pointer" />
                        </div>
                    </div>
                </Card.Header>
                <Card.Content class="space-y-6">
                    {#if isOAuth}
                        <div class="space-y-2">
                            <h3 class="text-sm font-medium">Connection</h3>
                            <p class="text-muted-foreground text-xs">
                                Connected via OAuth{data.principalEmail
                                    ? ` as ${data.principalEmail}`
                                    : ''}.
                            </p>
                            <Alert.Root>
                                <Info class="h-4 w-4" />
                                <Alert.Title>Managed by the connecting user</Alert.Title>
                                <Alert.Description>
                                    This source was set up via Google OAuth. To reconnect or change
                                    the authenticated account, the owner must manage it from their
                                    own Integrations settings.
                                </Alert.Description>
                            </Alert.Root>
                        </div>
                    {:else if isJwt}
                        <div class="space-y-4">
                            <div>
                                <h3 class="text-sm font-medium">Connection Settings</h3>
                                <p class="text-muted-foreground text-xs">
                                    Service account credentials used to impersonate Workspace users.
                                </p>
                            </div>
                            <GoogleServiceAccountForm
                                bind:serviceAccountJson
                                bind:principalEmail
                                bind:domain
                                hasStoredKey={data.hasStoredKey} />

                            {#if data.gmailSiblingId}
                                <Alert.Root>
                                    <Info class="h-4 w-4" />
                                    <Alert.Title>Gmail uses a separate credential copy</Alert.Title>
                                    <Alert.Description>
                                        Updating the service account key here only rotates it for
                                        Google Drive. If you want to rotate the key for Gmail too,
                                        open the Gmail source settings and replace it there as well.
                                    </Alert.Description>
                                </Alert.Root>
                            {/if}
                        </div>

                        <div class="space-y-4 border-t pt-6">
                            <div>
                                <h3 class="text-sm font-medium">User Access Control</h3>
                                <p class="text-muted-foreground text-xs">
                                    Control which Workspace users get their Drive files indexed.
                                </p>
                            </div>
                            <RadioGroup.Root
                                bind:value={userFilterMode}
                                name="userFilterMode"
                                disabled={!enabled}>
                                <div class="flex items-start space-x-3">
                                    <RadioGroup.Item value="all" id="all" />
                                    <Label for="all" class="cursor-pointer">
                                        <div>
                                            <div class="text-sm font-medium">All Users</div>
                                            <div class="text-muted-foreground text-xs">
                                                Index Drive files for all Google Workspace users
                                            </div>
                                        </div>
                                    </Label>
                                </div>

                                <div class="flex items-start space-x-3">
                                    <RadioGroup.Item value="whitelist" id="whitelist" />
                                    <Label for="whitelist" class="cursor-pointer">
                                        <div>
                                            <div class="text-sm font-medium">Specific Users</div>
                                            <div class="text-muted-foreground text-xs">
                                                Only index Drive files from selected users
                                            </div>
                                        </div>
                                    </Label>
                                </div>

                                <div class="flex items-start space-x-3">
                                    <RadioGroup.Item value="blacklist" id="blacklist" />
                                    <Label for="blacklist" class="cursor-pointer">
                                        <div>
                                            <div class="text-sm font-medium">Exclude Users</div>
                                            <div class="text-muted-foreground text-xs">
                                                Index all users except selected ones
                                            </div>
                                        </div>
                                    </Label>
                                </div>
                            </RadioGroup.Root>

                            {#if enabled && userFilterMode !== 'all'}
                                <div class="space-y-3 border-t pt-4">
                                    <div class="space-y-2">
                                        <div class="relative">
                                            <Search
                                                class="text-muted-foreground absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2" />
                                            <Input
                                                bind:value={searchQuery}
                                                oninput={handleSearchInput}
                                                placeholder="Search users..."
                                                class="px-10 py-1" />
                                            {#if isSearching}
                                                <Loader2
                                                    class="absolute top-1/2 right-3 h-4 w-4 -translate-y-1/2 animate-spin" />
                                            {/if}
                                        </div>

                                        {#if searchResults.length > 0}
                                            <div
                                                class="max-h-32 overflow-y-auto rounded-md border p-1">
                                                {#each searchResults.filter((user) => !selectedUsers.includes(user.email)) as user}
                                                    <button
                                                        type="button"
                                                        onclick={() => addUser(user.email)}
                                                        class="hover:bg-muted flex w-full items-center justify-between rounded px-2 py-1 text-left text-xs">
                                                        <div>
                                                            <div class="font-medium">
                                                                {user.name}
                                                            </div>
                                                            <div class="text-muted-foreground">
                                                                {user.email}
                                                            </div>
                                                        </div>
                                                        {#if user.isAdmin}
                                                            <Badge
                                                                variant="secondary"
                                                                class="text-xs">Admin</Badge>
                                                        {/if}
                                                    </button>
                                                {/each}
                                            </div>
                                        {/if}

                                        {#if selectedUsers.length > 0}
                                            <div class="space-y-2">
                                                <Label class="text-xs font-medium">
                                                    {userFilterMode === 'whitelist'
                                                        ? 'Included Users'
                                                        : 'Excluded Users'}
                                                </Label>
                                                <div class="flex flex-wrap gap-2">
                                                    {#each selectedUsers as email}
                                                        <div
                                                            class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors">
                                                            <span>{email}</span>
                                                            <button
                                                                type="button"
                                                                onclick={() => removeUser(email)}
                                                                class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                                                                aria-label="Remove {email}">
                                                                <X class="h-3 w-3" />
                                                            </button>
                                                        </div>
                                                    {/each}
                                                </div>
                                            </div>
                                        {/if}
                                    </div>
                                </div>
                            {/if}

                            {#each selectedUsers as email}
                                <input
                                    type="hidden"
                                    name={userFilterMode === 'whitelist'
                                        ? 'userWhitelist'
                                        : 'userBlacklist'}
                                    value={email} />
                            {/each}
                        </div>
                    {/if}
                </Card.Content>
                <Card.Footer class="flex justify-end">
                    <Button
                        type="submit"
                        disabled={isSubmitting || !hasUnsavedChanges}
                        class="cursor-pointer">
                        {#if isSubmitting}
                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                        {/if}
                        Save Configuration
                    </Button>
                </Card.Footer>
            </Card.Root>
        </form>

        <Card.Root>
            <Card.Content class="flex items-center justify-between">
                <div>
                    <Card.Title>Delete Source</Card.Title>
                    <Card.Description>
                        Permanently delete this source and all its synced documents, credentials,
                        and sync history.
                    </Card.Description>
                </div>
                <Button
                    variant="destructive"
                    class="cursor-pointer"
                    onclick={() => (showRemoveDialog = true)}>
                    Delete Permanently
                </Button>
            </Card.Content>
        </Card.Root>

        <RemoveSourceDialog
            bind:open={showRemoveDialog}
            sourceId={data.source.id}
            sourceName={data.source.name} />
    </div>
</div>
