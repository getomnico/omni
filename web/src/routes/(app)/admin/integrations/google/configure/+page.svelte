<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Badge } from '$lib/components/ui/badge'
    import { Search, X, AlertCircle, Loader2 } from '@lucide/svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import googleDriveLogo from '$lib/images/icons/google-drive.svg'
    import gmailLogo from '$lib/images/icons/gmail.svg'

    let { data }: PageProps = $props()

    // Service selection and settings
    let driveEnabled = $state(data.driveSource ? data.driveSource.isActive : false)
    let gmailEnabled = $state(data.gmailSource ? data.gmailSource.isActive : false)

    // User filtering for Drive
    let driveUserFilterMode = $state(data.driveSource?.userFilterMode || 'all')
    let driveSelectedUsers = $state<string[]>([])

    // User filtering for Gmail
    let gmailUserFilterMode = $state(data.gmailSource?.userFilterMode || 'all')
    let gmailSelectedUsers = $state<string[]>([])

    // User search state
    let searchQuery = $state('')
    let searchResults = $state<
        Array<{
            id: string
            email: string
            name: string
            orgUnit: string
            suspended: boolean
            isAdmin: boolean
        }>
    >([])
    let isSearching = $state(false)
    let searchDebounceTimer: ReturnType<typeof setTimeout>

    // Form state
    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)

    // Store reference to beforeunload handler for removal
    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    // Track original values for change detection (initialized from server data)
    let originalDriveEnabled = data.driveSource ? data.driveSource.isActive : false
    let originalGmailEnabled = data.gmailSource ? data.gmailSource.isActive : false
    let originalDriveUserFilterMode = data.driveSource?.userFilterMode || 'all'
    let originalGmailUserFilterMode = data.gmailSource?.userFilterMode || 'all'
    let originalDriveSelectedUsers: string[] = []
    let originalGmailSelectedUsers: string[] = []

    // Debounced search function
    async function searchUsers() {
        if (searchQuery.trim().length < 2) {
            searchResults = []
            return
        }

        isSearching = true
        try {
            // Use the first available source for the search (both use same credentials)
            const sourceId = data.driveSource?.id || data.gmailSource?.id
            if (!sourceId) {
                console.error('No source ID available for search')
                return
            }

            const params = new URLSearchParams({
                q: searchQuery,
                sourceId: sourceId,
                limit: '20',
            })

            const response = await fetch(`/api/integrations/google/users/search?${params}`)
            if (response.ok) {
                const result = await response.json()
                // Note: searchResults will need to be filtered based on context when displayed
                searchResults = result.users.filter((user: any) => !user.suspended)
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

    // Handle search input
    function handleSearchInput() {
        clearTimeout(searchDebounceTimer)
        searchDebounceTimer = setTimeout(() => {
            searchUsers()
        }, 300)
    }

    // Add user to selected list for specific service
    function addUser(email: string, service: 'drive' | 'gmail') {
        if (service === 'drive' && !driveSelectedUsers.includes(email)) {
            driveSelectedUsers = [...driveSelectedUsers, email]
        } else if (service === 'gmail' && !gmailSelectedUsers.includes(email)) {
            gmailSelectedUsers = [...gmailSelectedUsers, email]
        }
        searchQuery = ''
        searchResults = []
    }

    // Remove user from selected list for specific service
    function removeUser(email: string, service: 'drive' | 'gmail') {
        if (service === 'drive') {
            driveSelectedUsers = driveSelectedUsers.filter((u) => u !== email)
        } else {
            gmailSelectedUsers = gmailSelectedUsers.filter((u) => u !== email)
        }
    }

    // Validate form before submission
    function validateForm() {
        formErrors = []

        if (!driveEnabled && !gmailEnabled) {
            formErrors = [...formErrors, 'At least one service must be enabled']
            return false
        }

        if (
            driveEnabled &&
            driveUserFilterMode === 'whitelist' &&
            driveSelectedUsers.length === 0
        ) {
            formErrors = [...formErrors, 'Google Drive whitelist mode requires at least one user']
            return false
        }

        if (
            gmailEnabled &&
            gmailUserFilterMode === 'whitelist' &&
            gmailSelectedUsers.length === 0
        ) {
            formErrors = [...formErrors, 'Gmail whitelist mode requires at least one user']
            return false
        }

        return true
    }

    // Load existing settings
    onMount(() => {
        // Load Drive user filtering
        if (data.driveSource) {
            try {
                if (data.driveSource.userWhitelist) {
                    const whitelist =
                        typeof data.driveSource.userWhitelist === 'string'
                            ? JSON.parse(data.driveSource.userWhitelist)
                            : data.driveSource.userWhitelist
                    if (driveUserFilterMode === 'whitelist') {
                        driveSelectedUsers = Array.isArray(whitelist) ? whitelist : []
                        originalDriveSelectedUsers = [...driveSelectedUsers]
                    }
                }
                if (data.driveSource.userBlacklist) {
                    const blacklist =
                        typeof data.driveSource.userBlacklist === 'string'
                            ? JSON.parse(data.driveSource.userBlacklist)
                            : data.driveSource.userBlacklist
                    if (driveUserFilterMode === 'blacklist') {
                        driveSelectedUsers = Array.isArray(blacklist) ? blacklist : []
                        originalDriveSelectedUsers = [...driveSelectedUsers]
                    }
                }
            } catch (e) {
                console.error('Error parsing Drive user lists:', e)
            }
        }

        // Load Gmail user filtering
        if (data.gmailSource) {
            try {
                if (data.gmailSource.userWhitelist) {
                    const whitelist =
                        typeof data.gmailSource.userWhitelist === 'string'
                            ? JSON.parse(data.gmailSource.userWhitelist)
                            : data.gmailSource.userWhitelist
                    if (gmailUserFilterMode === 'whitelist') {
                        gmailSelectedUsers = Array.isArray(whitelist) ? whitelist : []
                        originalGmailSelectedUsers = [...gmailSelectedUsers]
                    }
                }
                if (data.gmailSource.userBlacklist) {
                    const blacklist =
                        typeof data.gmailSource.userBlacklist === 'string'
                            ? JSON.parse(data.gmailSource.userBlacklist)
                            : data.gmailSource.userBlacklist
                    if (gmailUserFilterMode === 'blacklist') {
                        gmailSelectedUsers = Array.isArray(blacklist) ? blacklist : []
                        originalGmailSelectedUsers = [...gmailSelectedUsers]
                    }
                }
            } catch (e) {
                console.error('Error parsing Gmail user lists:', e)
            }
        }

        // Set up beforeunload handler for unsaved changes
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

    // Handle SvelteKit navigation (back button, client-side routing)
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

    // Reactive statement to check for changes whenever form values change
    $effect(() => {
        // Access all reactive variables to make this effect run when they change
        const driveUsersChanged =
            JSON.stringify(driveSelectedUsers.sort()) !==
            JSON.stringify(originalDriveSelectedUsers.sort())
        const gmailUsersChanged =
            JSON.stringify(gmailSelectedUsers.sort()) !==
            JSON.stringify(originalGmailSelectedUsers.sort())

        hasUnsavedChanges =
            driveEnabled !== originalDriveEnabled ||
            gmailEnabled !== originalGmailEnabled ||
            driveUserFilterMode !== originalDriveUserFilterMode ||
            gmailUserFilterMode !== originalGmailUserFilterMode ||
            driveUsersChanged ||
            gmailUsersChanged
    })
</script>

<div class="container max-w-6xl py-8">
    <div class="mb-8">
        <h1 class="text-3xl font-bold">Configure Google Workspace</h1>
        <p class="text-muted-foreground mt-2">
            Configure Google Drive and Gmail services with independent user access controls
        </p>
    </div>

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
                // If the form submission was successful, disable unsaved changes check
                if (result.type === 'redirect') {
                    // Disable the unsaved changes check before navigation
                    skipUnsavedCheck = true
                    hasUnsavedChanges = false

                    // Remove the beforeunload event listener immediately
                    if (beforeUnloadHandler) {
                        window.removeEventListener('beforeunload', beforeUnloadHandler)
                        beforeUnloadHandler = null
                    }
                }

                await update()
                isSubmitting = false
            }
        }}
    >
        <div class="grid gap-6 md:grid-cols-2">
            <!-- Google Drive Configuration -->
            <Card.Root class="relative">
                <Card.Header>
                    <div class="flex items-start justify-between">
                        <div>
                            <Card.Title class="flex items-center gap-2">
                                <img src={googleDriveLogo} alt="Google Drive" class="h-5 w-5" />
                                Google Drive
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index documents, spreadsheets, presentations, and files
                            </Card.Description>
                        </div>
                        <div class="flex items-center gap-2">
                            <Label for="driveEnabled" class="text-sm">Enabled</Label>
                            <Switch
                                id="driveEnabled"
                                bind:checked={driveEnabled}
                                name="driveEnabled"
                            />
                        </div>
                    </div>
                </Card.Header>

                <Card.Content class="space-y-4">
                    <div class="space-y-4">
                        <div>
                            <Label class="text-sm font-medium">User Access Control</Label>
                        </div>

                        <RadioGroup.Root
                            bind:value={driveUserFilterMode}
                            name="driveUserFilterMode"
                            disabled={!driveEnabled}
                        >
                            <div class="flex items-start space-x-3">
                                <RadioGroup.Item value="all" id="drive-all" />
                                <Label for="drive-all" class="cursor-pointer">
                                    <div>
                                        <div class="text-sm font-medium">All Users</div>
                                        <div class="text-muted-foreground text-xs">
                                            Index Drive files for all Google Workspace users
                                        </div>
                                    </div>
                                </Label>
                            </div>

                            <div class="flex items-start space-x-3">
                                <RadioGroup.Item value="whitelist" id="drive-whitelist" />
                                <Label for="drive-whitelist" class="cursor-pointer">
                                    <div>
                                        <div class="text-sm font-medium">Specific Users</div>
                                        <div class="text-muted-foreground text-xs">
                                            Only index Drive files from selected users
                                        </div>
                                    </div>
                                </Label>
                            </div>

                            <div class="flex items-start space-x-3">
                                <RadioGroup.Item value="blacklist" id="drive-blacklist" />
                                <Label for="drive-blacklist" class="cursor-pointer">
                                    <div>
                                        <div class="text-sm font-medium">Exclude Users</div>
                                        <div class="text-muted-foreground text-xs">
                                            Index all users except selected ones
                                        </div>
                                    </div>
                                </Label>
                            </div>
                        </RadioGroup.Root>

                        {#if driveEnabled && driveUserFilterMode !== 'all'}
                            <div class="space-y-3 border-t pt-4">
                                <div class="space-y-2">
                                    <div class="relative">
                                        <Search
                                            class="text-muted-foreground absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2"
                                        />
                                        <input
                                            type="text"
                                            bind:value={searchQuery}
                                            oninput={handleSearchInput}
                                            placeholder="Search users..."
                                            class="border-input bg-background ring-offset-background placeholder:text-muted-foreground focus-visible:ring-ring flex h-9 w-full rounded-md border px-10 py-1 text-sm focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none"
                                        />
                                        {#if isSearching}
                                            <Loader2
                                                class="absolute top-1/2 right-3 h-4 w-4 -translate-y-1/2 animate-spin"
                                            />
                                        {/if}
                                    </div>

                                    {#if searchResults.length > 0}
                                        <div class="max-h-32 overflow-y-auto rounded-md border p-1">
                                            {#each searchResults.filter((user) => !driveSelectedUsers.includes(user.email)) as user}
                                                <button
                                                    type="button"
                                                    onclick={() => addUser(user.email, 'drive')}
                                                    class="hover:bg-muted flex w-full items-center justify-between rounded px-2 py-1 text-left text-xs"
                                                >
                                                    <div>
                                                        <div class="font-medium">{user.name}</div>
                                                        <div class="text-muted-foreground">
                                                            {user.email}
                                                        </div>
                                                    </div>
                                                    {#if user.isAdmin}
                                                        <Badge variant="secondary" class="text-xs"
                                                            >Admin</Badge
                                                        >
                                                    {/if}
                                                </button>
                                            {/each}
                                        </div>
                                    {/if}

                                    {#if driveSelectedUsers.length > 0}
                                        <div class="space-y-2">
                                            <Label class="text-xs font-medium">
                                                {driveUserFilterMode === 'whitelist'
                                                    ? 'Included Users'
                                                    : 'Excluded Users'}
                                            </Label>
                                            <div class="flex flex-wrap gap-2">
                                                {#each driveSelectedUsers as email}
                                                    <div
                                                        class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors"
                                                    >
                                                        <span>{email}</span>
                                                        <button
                                                            type="button"
                                                            onclick={() =>
                                                                removeUser(email, 'drive')}
                                                            class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                                                            aria-label="Remove {email}"
                                                        >
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
                    </div>

                    <!-- Hidden inputs for Drive user filtering -->
                    {#each driveSelectedUsers as email}
                        <input
                            type="hidden"
                            name={driveUserFilterMode === 'whitelist'
                                ? 'driveUserWhitelist'
                                : 'driveUserBlacklist'}
                            value={email}
                        />
                    {/each}
                </Card.Content>
            </Card.Root>

            <!-- Gmail Configuration -->
            <Card.Root class="relative">
                <Card.Header>
                    <div class="flex items-start justify-between">
                        <div>
                            <Card.Title class="flex items-center gap-2">
                                <img src={gmailLogo} alt="Gmail" class="h-5 w-5" />
                                Gmail
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index email threads and attachments
                            </Card.Description>
                        </div>
                        <div class="flex items-center gap-2">
                            <Label for="gmailEnabled" class="text-sm">Enabled</Label>
                            <Switch
                                id="gmailEnabled"
                                bind:checked={gmailEnabled}
                                name="gmailEnabled"
                            />
                        </div>
                    </div>
                </Card.Header>

                <Card.Content class="space-y-4">
                    <div class="space-y-4">
                        <div>
                            <Label class="text-sm font-medium">User Access Control</Label>
                        </div>

                        <RadioGroup.Root
                            bind:value={gmailUserFilterMode}
                            name="gmailUserFilterMode"
                            disabled={!gmailEnabled}
                        >
                            <div class="flex items-start space-x-3">
                                <RadioGroup.Item value="all" id="gmail-all" />
                                <Label for="gmail-all" class="cursor-pointer">
                                    <div>
                                        <div class="text-sm font-medium">All Users</div>
                                        <div class="text-muted-foreground text-xs">
                                            Index emails for all Google Workspace users
                                        </div>
                                    </div>
                                </Label>
                            </div>

                            <div class="flex items-start space-x-3">
                                <RadioGroup.Item value="whitelist" id="gmail-whitelist" />
                                <Label for="gmail-whitelist" class="cursor-pointer">
                                    <div>
                                        <div class="text-sm font-medium">Specific Users</div>
                                        <div class="text-muted-foreground text-xs">
                                            Only index emails from selected users
                                        </div>
                                    </div>
                                </Label>
                            </div>

                            <div class="flex items-start space-x-3">
                                <RadioGroup.Item value="blacklist" id="gmail-blacklist" />
                                <Label for="gmail-blacklist" class="cursor-pointer">
                                    <div>
                                        <div class="text-sm font-medium">Exclude Users</div>
                                        <div class="text-muted-foreground text-xs">
                                            Index all users except selected ones
                                        </div>
                                    </div>
                                </Label>
                            </div>
                        </RadioGroup.Root>

                        {#if gmailEnabled && gmailUserFilterMode !== 'all'}
                            <div class="space-y-3 border-t pt-4">
                                <div class="space-y-2">
                                    <div class="relative">
                                        <Search
                                            class="text-muted-foreground absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2"
                                        />
                                        <input
                                            type="text"
                                            bind:value={searchQuery}
                                            oninput={handleSearchInput}
                                            placeholder="Search users..."
                                            class="border-input bg-background ring-offset-background placeholder:text-muted-foreground focus-visible:ring-ring flex h-9 w-full rounded-md border px-10 py-1 text-sm focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none"
                                        />
                                        {#if isSearching}
                                            <Loader2
                                                class="absolute top-1/2 right-3 h-4 w-4 -translate-y-1/2 animate-spin"
                                            />
                                        {/if}
                                    </div>

                                    {#if searchResults.length > 0}
                                        <div class="max-h-32 overflow-y-auto rounded-md border p-1">
                                            {#each searchResults.filter((user) => !gmailSelectedUsers.includes(user.email)) as user}
                                                <button
                                                    type="button"
                                                    onclick={() => addUser(user.email, 'gmail')}
                                                    class="hover:bg-muted flex w-full items-center justify-between rounded px-2 py-1 text-left text-xs"
                                                >
                                                    <div>
                                                        <div class="font-medium">{user.name}</div>
                                                        <div class="text-muted-foreground">
                                                            {user.email}
                                                        </div>
                                                    </div>
                                                    {#if user.isAdmin}
                                                        <Badge variant="secondary" class="text-xs"
                                                            >Admin</Badge
                                                        >
                                                    {/if}
                                                </button>
                                            {/each}
                                        </div>
                                    {/if}

                                    {#if gmailSelectedUsers.length > 0}
                                        <div class="space-y-2">
                                            <Label class="text-xs font-medium">
                                                {gmailUserFilterMode === 'whitelist'
                                                    ? 'Included Users'
                                                    : 'Excluded Users'}
                                            </Label>
                                            <div class="flex flex-wrap gap-2">
                                                {#each gmailSelectedUsers as email}
                                                    <div
                                                        class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors"
                                                    >
                                                        <span>{email}</span>
                                                        <button
                                                            type="button"
                                                            onclick={() =>
                                                                removeUser(email, 'gmail')}
                                                            class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                                                            aria-label="Remove {email}"
                                                        >
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
                    </div>

                    <!-- Hidden inputs for Gmail user filtering -->
                    {#each gmailSelectedUsers as email}
                        <input
                            type="hidden"
                            name={gmailUserFilterMode === 'whitelist'
                                ? 'gmailUserWhitelist'
                                : 'gmailUserBlacklist'}
                            value={email}
                        />
                    {/each}
                </Card.Content>
            </Card.Root>
        </div>

        <!-- Submit buttons -->
        <div class="mt-8 flex justify-between">
            <Button variant="outline" href="/admin/integrations">Cancel</Button>
            <Button type="submit" disabled={isSubmitting || !hasUnsavedChanges}>
                {#if isSubmitting}
                    <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                {/if}
                Save Configuration
            </Button>
        </div>
    </form>
</div>
