<script lang="ts">
    import { Label } from '$lib/components/ui/label'
    import { Input } from '$lib/components/ui/input'
    import { X, Loader2, Search, AlertCircle, RefreshCw } from '@lucide/svelte'
    import * as Alert from '$lib/components/ui/alert'
    import { onDestroy } from 'svelte'
    import type { FolderPathFilter } from '$lib/types'
    import type { DriveFolderDiscoveryEntry, DriveFolderDiscoveryResponse } from '$lib/types/search'

    let {
        sourceId = '',
        serviceAccountJson = '',
        principalEmail = '',
        domain = '',
        selected = $bindable([] as FolderPathFilter[]),
        disabled = false,
        label = 'Folders to index',
        description = 'Select specific shared drives or top-level folders to index. Leave empty to index everything.',
    }: {
        sourceId?: string
        serviceAccountJson?: string
        principalEmail?: string
        domain?: string
        selected?: FolderPathFilter[]
        disabled?: boolean
        label?: string
        description?: string
    } = $props()

    // Internal state
    let searchQuery = $state('')
    let allItems = $state<DriveFolderDiscoveryEntry[]>([])
    let filteredItems = $state<DriveFolderDiscoveryEntry[]>([])
    let showDropdown = $state(false)
    let isLoading = $state(false)
    let hasLoaded = $state(false)
    let errorMessage = $state('')
    let inputRef = $state<HTMLInputElement | null>(null)

    // Guard: only start auto-discovery after mount, and only when credential
    // context actually changes (not on first mount where persisted selected
    // items are already present).
    let hasInitialized = $state(false)

    // Generation counter: bumped on every credential-context change.
    // Each async operation captures the generation at start and verifies it
    // before any state mutation after an await.
    let generation = $state(0)

    // Request-level counter: bumped on EVERY discoverFolders invocation.
    // Only the request whose version matches `currentRequestVersion` may
    // mutate state after an await, preventing races between sequential calls
    // with the same credential generation.
    let currentRequestVersion = $state(0)

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
            sid: sourceId,
        }),
    )

    // Track the previous context token to detect changes.
    let prevContextToken = $state('')

    // Reactive effect: detect credential-context change and re-discover.
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

        // Cancel any in-flight request.
        pendingRequest?.abort()
        pendingRequest = null

        // Clear selected items ONLY on an actual credential-context change
        // after initialization. On initial mount, pre-loaded selected items
        // from persisted config are preserved.
        selected = []

        // Reset discovery state.
        searchQuery = ''
        allItems = []
        filteredItems = []
        hasLoaded = false
        errorMessage = ''

        // Automatically trigger discovery for the new context.
        discoverFolders(generation)
    })

    let pendingRequest: AbortController | null = null

    // Load folders when credentials are available.
    // Takes `gen` (credential generation) and `rv` (request version) captured
    // at call time for race-safe post-await checks.
    //
    // Every invocation gets a unique request version. Only the call whose
    // `rv` matches `currentRequestVersion` may mutate state after an await,
    // preventing races between sequential calls with the same credential gen.
    async function discoverFolders(gen: number, rv?: number) {
        // Determine request version for this invocation.
        if (rv === undefined) {
            currentRequestVersion += 1
            rv = currentRequestVersion
        }

        if (!serviceAccountJson || !principalEmail || !domain) {
            if (!sourceId) {
                isLoading = false
                return
            }
            // Stored-credential path: use sourceId even without transient creds.
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

        const signal = controller.signal

        try {
            let response: Response

            if (serviceAccountJson && principalEmail && domain) {
                // Use preview API with transient credentials
                response = await fetch('/api/preview-action', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        sourceType: 'google_drive',
                        action: 'discover_folders',
                        params: {},
                        serviceAccountJson,
                        principalEmail,
                        domain,
                    }),
                    signal,
                })
            } else if (sourceId) {
                // Use stored-credential action API
                response = await fetch(`/api/sources/${sourceId}/action`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        action: 'discover_folders',
                        params: {},
                    }),
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
                if (statusOk && items.length > 0) {
                    allItems = items
                    filteredItems = items
                    hasLoaded = true
                    if (searchQuery.trim()) {
                        filterItems(searchQuery)
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
            filteredItems = allItems
            return
        }
        const q = query.trim().toLowerCase()
        filteredItems = allItems.filter(
            (item) =>
                !selected.some((s) => s.id === item.id) &&
                (item.name.toLowerCase().includes(q) ||
                    item.path.toLowerCase().includes(q) ||
                    item.driveId.toLowerCase().includes(q)),
        )
    }

    function handleInput() {
        if (!hasLoaded && !isLoading && !errorMessage) {
            // Auto-discover on first input interaction
            discoverFolders(generation)
        }
        filterItems(searchQuery)
        showDropdown = filteredItems.length > 0 && searchQuery.trim().length > 0
    }

    function selectItem(item: DriveFolderDiscoveryEntry) {
        if (selected.some((s) => s.id === item.id)) return
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
        searchQuery = ''
        showDropdown = false
        filteredItems = allItems
        inputRef?.focus()
    }

    function removeItem(id: string) {
        selected = selected.filter((s) => s.id !== id)
    }

    function handleRetry() {
        // Reset state and trigger a fresh discovery with the current generation.
        allItems = []
        filteredItems = []
        hasLoaded = false
        errorMessage = ''
        discoverFolders(generation)
    }

    // Clean up in-flight requests on destroy
    onDestroy(() => {
        pendingRequest?.abort()
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
            searchQuery.trim().length > 0 &&
            filteredItems.length > 0 &&
            !showDropdown
        ) {
            showDropdown = true
        }
        if (!hasLoaded && !isLoading && !errorMessage) {
            discoverFolders(generation)
        }
    }
</script>

<div class="space-y-2">
    <Label class="text-sm font-medium">{label}</Label>
    <p class="text-muted-foreground text-xs">{description}</p>

    <!-- Loading state -->
    {#if isLoading && !hasLoaded}
        <div class="text-muted-foreground flex items-center gap-2 py-2 text-sm">
            <Loader2 class="h-4 w-4 animate-spin" />
            <span>Discovering shared drives and folders...</span>
        </div>
    {/if}

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

    <!-- Empty state (discovered but no items) -->
    {#if hasLoaded && allItems.length === 0 && !isLoading && !errorMessage}
        <p class="text-muted-foreground py-1 text-sm italic">
            No shared drives found. Make sure the service account has access to shared drives in
            this domain.
        </p>
    {/if}

    <!-- Search / typeahead -->
    <div class="relative" class:opacity-50={disabled}>
        <Search class="text-muted-foreground absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2" />
        <Input
            bind:ref={inputRef}
            bind:value={searchQuery}
            oninput={handleInput}
            onfocus={handleFocus}
            onblur={handleBlur}
            placeholder="Search folders..."
            class="px-10 py-1"
            {disabled} />
        {#if isLoading}
            <Loader2 class="absolute top-1/2 right-3 h-4 w-4 -translate-y-1/2 animate-spin" />
        {/if}

        <!-- Dropdown -->
        {#if showDropdown && filteredItems.length > 0}
            <div
                class="absolute z-50 mt-1 max-h-48 w-full overflow-y-auto rounded-md border bg-white shadow-lg dark:bg-slate-950">
                {#each filteredItems as item (item.id)}
                    <button
                        type="button"
                        onclick={() => selectItem(item)}
                        class="hover:bg-muted flex w-full items-start gap-2 px-3 py-2 text-left text-sm">
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
            </div>
        {:else if showDropdown && searchQuery.trim().length > 0 && !isLoading}
            <div
                class="text-muted-foreground absolute z-50 mt-1 w-full rounded-md border bg-white px-3 py-2 text-sm shadow-lg dark:bg-slate-950">
                No matching folders
            </div>
        {/if}
    </div>

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
