<script lang="ts">
    import { Label } from '$lib/components/ui/label'
    import { Input } from '$lib/components/ui/input'
    import { X, Loader2, Search, AlertCircle, RefreshCw } from '@lucide/svelte'
    import * as Alert from '$lib/components/ui/alert'
    import * as Popover from '$lib/components/ui/popover'
    import { onDestroy } from 'svelte'
    import type { FolderPathFilter } from '$lib/types'
    import type { DriveFolderDiscoveryEntry, DriveFolderDiscoveryResponse } from '$lib/types/search'

    let {
        sourceId = '',
        serviceAccountJson = '',
        principalEmail = '',
        domain = '',
        authMode = 'domain_wide_delegation' as 'domain_wide_delegation' | 'service_account_direct',
        selected = $bindable([] as FolderPathFilter[]),
        disabled = false,
        label = 'Folders to index',
        description = 'Select specific My Drive folders, shared drives, or shared-drive folders. Leave empty to index everything.',
        onSelectedChange = undefined as ((selected: FolderPathFilter[]) => void) | undefined,
        action = 'discover_folders',
    }: {
        sourceId?: string
        serviceAccountJson?: string
        principalEmail?: string
        domain?: string
        authMode?: 'domain_wide_delegation' | 'service_account_direct'
        selected?: FolderPathFilter[]
        disabled?: boolean
        label?: string
        description?: string
        onSelectedChange?: (selected: FolderPathFilter[]) => void
        action?: 'discover_folders' | 'discover_personal_folders'
    } = $props()

    const isSaDirect = $derived(authMode === 'service_account_direct')

    // Internal state
    let searchQuery = $state('')
    let allItems = $state<DriveFolderDiscoveryEntry[]>([])
    let filteredItems = $state<DriveFolderDiscoveryEntry[]>([])
    let showDropdown = $state(false)
    let isLoading = $state(false)
    let hasLoaded = $state(false)
    let errorMessage = $state('')
    let discoveryNotice = $state('')
    let inputRef = $state<HTMLInputElement | null>(null)
    let dropdownAnchor = $state<HTMLElement | null>(null)

    // Guard: preserve persisted selections on the initial mount. Discovery is
    // search-driven and never runs automatically.
    let hasInitialized = $state(false)

    // Generation counter: bumped on every credential-context change.
    // Each async operation captures the generation at start and verifies it
    // before any state mutation after an await.
    let generation = $state(0)

    // Request-level counter: only the request whose version matches
    // `currentRequestVersion` may mutate state after an await, preventing
    // races between sequential searches with the same credential generation.
    let currentRequestVersion = $state(0)
    let lastDiscoveryQuery = $state('')

    // Build a credential-context token from all relevant inputs.
    // Uses the raw values rather than a hash to guarantee no collisions,
    // and includes sourceId as fallback for stored-credential mode.
    // PrincipalEmail/domain changes count even when key is blank
    // because they are part of the credential context.
    let contextToken = $derived(
        JSON.stringify({
            sa: serviceAccountJson,
            pe: principalEmail,
            dm: domain,
            am: authMode,
            sid: sourceId,
        }),
    )

    // Track the previous context token to detect changes.
    let prevContextToken = $state('')

    // Reactive effect: reset discovery state when the credential context changes.
    $effect(() => {
        // First evaluation: set up initial state without clearing selections.
        if (!hasInitialized) {
            hasInitialized = true
            prevContextToken = contextToken
            return
        }

        // Subsequent evaluations: detect actual changes.
        if (contextToken === prevContextToken) return
        prevContextToken = contextToken

        // Bump generation to invalidate any in-flight responses.
        generation += 1

        // Cancel any in-flight request or pending search debounce.
        pendingRequest?.abort()
        pendingRequest = null
        currentRequestVersion += 1
        if (searchDebounce) clearTimeout(searchDebounce)
        searchDebounce = undefined

        // Clear selected items ONLY on an actual credential-context change
        // after initialization. On initial mount, pre-loaded selected items
        // from persisted config are preserved.
        selected = []
        onSelectedChange?.(selected)

        // Reset discovery state.
        searchQuery = ''
        allItems = []
        filteredItems = []
        hasLoaded = false
        errorMessage = ''
        discoveryNotice = ''
        lastDiscoveryQuery = ''

        // Discovery starts only after the user enters a search prefix.
    })

    let pendingRequest: AbortController | null = null
    let searchDebounce: ReturnType<typeof setTimeout> | undefined
    const minRemoteSearchLength = 2

    function queryLength(query: string): number {
        return Array.from(query.trim()).length
    }

    // Search accessible folders and drives for a user-entered prefix.
    async function discoverFolders(gen: number, query: string) {
        const normalizedQuery = query.trim()
        if (queryLength(normalizedQuery) < minRemoteSearchLength) return

        lastDiscoveryQuery = normalizedQuery
        currentRequestVersion += 1
        const rv = currentRequestVersion

        if (!serviceAccountJson) {
            if (!sourceId) {
                isLoading = false
                return
            }
            // Stored-credential path: use sourceId even without transient creds.
        }

        // SA-direct needs only the SA key (no principal/domain); DWD needs all.
        if (!isSaDirect && serviceAccountJson && (!principalEmail || !domain)) {
            isLoading = false
            return
        }

        // Abort any prior in-flight request.
        pendingRequest?.abort()
        const controller = new AbortController()
        pendingRequest = controller

        isLoading = true
        errorMessage = ''
        allItems = []
        filteredItems = []
        hasLoaded = false
        discoveryNotice = ''

        const signal = controller.signal

        try {
            let response: Response

            const actionParams = {
                ...(isSaDirect ? { auth_mode: 'service_account_direct' } : {}),
                query: normalizedQuery,
            }

            if (serviceAccountJson && (isSaDirect || (principalEmail && domain))) {
                // Invoke the connector directly with transient credentials.
                response = await fetch('/api/connectors/google_drive/action', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        action,
                        params: actionParams,
                        serviceAccountJson,
                        principalEmail,
                        domain,
                        authMode,
                    }),
                    signal,
                })
            } else if (sourceId) {
                // Use stored-credential action API
                response = await fetch(`/api/sources/${sourceId}/action`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ action, params: actionParams }),
                    signal,
                })
            } else {
                // No credentials available at all
                isLoading = false
                return
            }

            // RACE CHECK: if generation or request version has changed, discard.
            if (gen !== generation || rv !== currentRequestVersion) return

            if (response.ok) {
                const body: { status?: string; result?: DriveFolderDiscoveryResponse } =
                    await response.json()

                // RACE CHECK
                if (gen !== generation || rv !== currentRequestVersion) return

                const statusOk = body?.status === 'ok' || body?.status === 'success'
                const items = body?.result?.items ?? []
                discoveryNotice = body.result?.truncated
                    ? 'Results are limited. Refine your search to see more folders or drives.'
                    : ''
                if (statusOk && items.length > 0) {
                    allItems = items
                    filteredItems = items
                    hasLoaded = true
                    if (searchQuery.trim()) {
                        filterItems(searchQuery)
                        showDropdown = filteredItems.length > 0
                    }
                } else if (statusOk && items.length === 0) {
                    hasLoaded = true
                    errorMessage = ''
                } else {
                    errorMessage = 'No folders could be discovered.'
                }
            } else {
                let msg = 'Failed to discover folders'
                try {
                    const errBody = await response.json()
                    if (gen !== generation || rv !== currentRequestVersion) return
                    msg = errBody.error || errBody.message || msg
                } catch {
                    if (gen !== generation || rv !== currentRequestVersion) return
                    msg = (await response.text()) || msg
                }
                errorMessage = msg
            }
        } catch (err) {
            if (gen !== generation || rv !== currentRequestVersion) return
            if (err instanceof DOMException && err.name === 'AbortError') {
                return
            }
            console.error('Error discovering folders:', err)
            errorMessage = 'Network error. Please check your connection and try again.'
        } finally {
            if (gen === generation && rv === currentRequestVersion) {
                isLoading = false
            }
        }
    }

    function filterItems(query: string) {
        if (!query.trim()) {
            filteredItems = isSaDirect
                ? allItems.filter((item) => item.kind === 'shared_drive_root')
                : allItems
            return
        }
        const q = query.trim().toLowerCase()
        filteredItems = (
            isSaDirect ? allItems.filter((item) => item.kind === 'shared_drive_root') : allItems
        ).filter(
            (item) =>
                !selected.some((s) => s.id === item.id) && item.name.toLowerCase().startsWith(q),
        )
    }

    function handleInput() {
        const query = searchQuery.trim()
        if (
            queryLength(query) >= minRemoteSearchLength &&
            query === lastDiscoveryQuery &&
            (isLoading || hasLoaded)
        ) {
            if (hasLoaded) {
                filterItems(query)
                showDropdown = filteredItems.length > 0
            }
            return
        }

        if (searchDebounce) clearTimeout(searchDebounce)
        searchDebounce = undefined
        pendingRequest?.abort()
        pendingRequest = null
        currentRequestVersion += 1
        isLoading = false
        errorMessage = ''
        discoveryNotice = ''
        showDropdown = false

        if (queryLength(query) < minRemoteSearchLength) {
            // Short or cleared input never triggers discovery.
            lastDiscoveryQuery = ''
            allItems = []
            filteredItems = []
            hasLoaded = false
            return
        }

        allItems = []
        filteredItems = []
        hasLoaded = false

        // Search the full accessible hierarchy after the user pauses typing.
        searchDebounce = setTimeout(() => {
            discoverFolders(generation, query)
            searchDebounce = undefined
        }, 250)
    }

    function selectItem(item: DriveFolderDiscoveryEntry) {
        if (selected.some((s) => s.id === item.id)) return
        // SA-direct v1 is whole-drives only — never select folder-kind items.
        if (isSaDirect && item.kind !== 'shared_drive_root') return
        selected = [
            ...selected,
            {
                id: item.id,
                name: item.name,
                path: item.path,
                driveId: item.driveId,
                kind: item.kind === 'folder' ? 'folder' : 'shared_drive_root',
            },
        ]
        onSelectedChange?.(selected)
        searchQuery = ''
        showDropdown = false
        filteredItems = allItems
        inputRef?.focus()
    }

    function removeItem(id: string) {
        selected = selected.filter((s) => s.id !== id)
        onSelectedChange?.(selected)
    }

    function handleRetry() {
        // Reset state and trigger a fresh discovery with the current generation.
        allItems = []
        filteredItems = []
        hasLoaded = false
        errorMessage = ''
        discoveryNotice = ''
        const query = searchQuery.trim()
        if (queryLength(query) >= minRemoteSearchLength) {
            discoverFolders(generation, query)
        }
    }

    // Clean up in-flight requests on destroy
    onDestroy(() => {
        pendingRequest?.abort()
        if (searchDebounce) clearTimeout(searchDebounce)
    })

    function handleBlur() {
        // Delay hiding so click on dropdown item registers
        setTimeout(() => {
            showDropdown = false
        }, 200)
    }

    function handleFocus() {
        if (
            hasLoaded &&
            queryLength(searchQuery) >= minRemoteSearchLength &&
            filteredItems.length > 0 &&
            !showDropdown
        ) {
            showDropdown = true
        }
    }
</script>

<div class="space-y-2">
    <Label class="text-sm font-medium">{label}</Label>
    <p class="text-muted-foreground text-xs">{description} Search uses the beginning of a name.</p>

    <!-- Error state -->
    {#if errorMessage && !isLoading}
        <Alert.Root variant="destructive" class="mb-2">
            <AlertCircle class="h-4 w-4" />
            <Alert.Title>Discovery Failed</Alert.Title>
            <Alert.Description>
                <p class="text-sm">{errorMessage}</p>
                <button
                    type="button"
                    onclick={handleRetry}
                    class="mt-1 inline-flex cursor-pointer items-center gap-1 text-sm underline hover:no-underline">
                    <RefreshCw class="h-3 w-3" />
                    Retry
                </button>
            </Alert.Description>
        </Alert.Root>
    {/if}

    {#if discoveryNotice && !isLoading}
        <p class="text-muted-foreground py-1 text-xs">{discoveryNotice}</p>
    {/if}

    <!-- Empty state (discovered but no items) -->
    {#if hasLoaded && allItems.length === 0 && !isLoading && !errorMessage}
        <p class="text-muted-foreground py-1 text-sm italic">
            No folders or shared drives found. Make sure this account has access to the Drive
            content you want to index.
        </p>
    {/if}

    <!-- Search / typeahead -->
    <Popover.Root bind:open={showDropdown}>
        <div bind:this={dropdownAnchor} class="relative" class:opacity-50={disabled}>
            <Search
                class="text-muted-foreground absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2" />
            <Input
                bind:ref={inputRef}
                bind:value={searchQuery}
                oninput={handleInput}
                onfocus={handleFocus}
                onblur={handleBlur}
                placeholder="Search drives and folders..."
                class="px-10 py-1"
                {disabled} />
            {#if isLoading}
                <Loader2 class="absolute top-1/2 right-3 h-4 w-4 -translate-y-1/2 animate-spin" />
            {/if}
        </div>

        {#if showDropdown && filteredItems.length > 0}
            <Popover.Content
                customAnchor={dropdownAnchor}
                align="start"
                sideOffset={4}
                trapFocus={false}
                class="max-h-48 w-[var(--bits-popover-anchor-width)] overflow-y-auto p-0"
                onOpenAutoFocus={(event) => event.preventDefault()}
                onCloseAutoFocus={(event) => event.preventDefault()}>
                {#each filteredItems as item (item.id)}
                    <button
                        type="button"
                        onmousedown={(event) => event.preventDefault()}
                        onclick={() => selectItem(item)}
                        class="hover:bg-muted flex w-full cursor-pointer items-start gap-2 px-3 py-2 text-left text-sm">
                        <div class="min-w-0 flex-1">
                            <div class="truncate font-medium">{item.name}</div>
                            <div class="text-muted-foreground truncate text-xs">{item.path}</div>
                        </div>
                        <span
                            class="mt-0.5 shrink-0 rounded bg-slate-100 px-1.5 py-0.5 text-[10px] text-slate-600 uppercase dark:bg-slate-800 dark:text-slate-300">
                            {item.kind === 'shared_drive_root' ? 'Drive' : 'Folder'}
                        </span>
                    </button>
                {/each}
            </Popover.Content>
        {/if}
    </Popover.Root>

    <!-- Selected items as chips -->
    {#if selected.length > 0}
        <div class="flex flex-wrap gap-2 pt-1">
            {#each selected as item (item.id)}
                <div
                    class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors">
                    <span class="max-w-40 truncate" title={item.path}>{item.name}</span>
                    {#if !disabled}
                        <button
                            type="button"
                            onclick={() => removeItem(item.id)}
                            class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                            aria-label="Remove {item.name}">
                            <X class="h-3 w-3" />
                        </button>
                    {/if}
                </div>
            {/each}
        </div>
    {:else if !isLoading && hasLoaded}
        <p class="text-muted-foreground text-xs italic">
            No folders selected — all accessible files will be indexed.
        </p>
    {/if}
</div>
