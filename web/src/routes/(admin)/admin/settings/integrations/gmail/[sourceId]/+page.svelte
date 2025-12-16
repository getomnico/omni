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
    import gmailLogo from '$lib/images/icons/gmail.svg'

    let { data }: PageProps = $props()

    let enabled = $state(data.source.isActive)
    let userFilterMode = $state(data.source.userFilterMode || 'all')
    let selectedUsers = $state<string[]>([])

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

    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    let originalEnabled = data.source.isActive
    let originalUserFilterMode = data.source.userFilterMode || 'all'
    let originalSelectedUsers: string[] = []

    async function searchUsers() {
        if (searchQuery.trim().length < 2) {
            searchResults = []
            return
        }

        isSearching = true
        try {
            const params = new URLSearchParams({
                q: searchQuery,
                sourceId: data.source.id,
                limit: '20',
            })

            const response = await fetch(`/api/integrations/google/users/search?${params}`)
            if (response.ok) {
                const result = await response.json()
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

        if (enabled && userFilterMode === 'whitelist' && selectedUsers.length === 0) {
            formErrors = [...formErrors, 'Whitelist mode requires at least one user']
            return false
        }

        return true
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
        const usersChanged =
            JSON.stringify(selectedUsers.sort()) !== JSON.stringify(originalSelectedUsers.sort())

        hasUnsavedChanges =
            enabled !== originalEnabled || userFilterMode !== originalUserFilterMode || usersChanged
    })
</script>

<svelte:head>
    <title>Configure Gmail - {data.source.name}</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Configure Gmail</h1>
            <p class="text-muted-foreground mt-2">
                Configure Gmail indexing with user access controls
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
            }}>
            <Card.Root class="relative">
                <Card.Header>
                    <div class="flex items-start justify-between">
                        <div>
                            <Card.Title class="flex items-center gap-2">
                                <img src={gmailLogo} alt="Gmail" class="h-5 w-5" />
                                {data.source.name}
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index email threads and attachments
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

                <Card.Content class="space-y-4">
                    <div class="space-y-4">
                        <div>
                            <Label class="text-sm font-medium">User Access Control</Label>
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
                                            Index emails for all Google Workspace users
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
                                            Only index emails from selected users
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
                                        <input
                                            type="text"
                                            bind:value={searchQuery}
                                            oninput={handleSearchInput}
                                            placeholder="Search users..."
                                            class="border-input bg-background ring-offset-background placeholder:text-muted-foreground focus-visible:ring-ring flex h-9 w-full rounded-md border px-10 py-1 text-sm focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none" />
                                        {#if isSearching}
                                            <Loader2
                                                class="absolute top-1/2 right-3 h-4 w-4 -translate-y-1/2 animate-spin" />
                                        {/if}
                                    </div>

                                    {#if searchResults.length > 0}
                                        <div class="max-h-32 overflow-y-auto rounded-md border p-1">
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
                                                        <Badge variant="secondary" class="text-xs"
                                                            >Admin</Badge>
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
                    </div>

                    {#each selectedUsers as email}
                        <input
                            type="hidden"
                            name={userFilterMode === 'whitelist'
                                ? 'userWhitelist'
                                : 'userBlacklist'}
                            value={email} />
                    {/each}
                </Card.Content>
            </Card.Root>

            <div class="mt-8 flex justify-between">
                <Button variant="outline" href="/admin/settings/integrations">Cancel</Button>
                <Button
                    type="submit"
                    disabled={isSubmitting || !hasUnsavedChanges}
                    class="cursor-pointer">
                    {#if isSubmitting}
                        <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                    {/if}
                    Save Configuration
                </Button>
            </div>
        </form>
    </div>
</div>
