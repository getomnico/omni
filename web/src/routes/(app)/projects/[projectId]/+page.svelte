<script lang="ts">
    import { goto, invalidateAll } from '$app/navigation'
    import { resolve } from '$app/paths'
    import { toast } from 'svelte-sonner'
    import {
        Archive,
        ArchiveRestore,
        ArrowLeft,
        ChevronRight,
        Ellipsis,
        FileText,
        Folder,
        Layers,
        MessageSquare,
        Pencil,
        Plus,
        Search,
        Save,
        Trash2,
        X,
    } from '@lucide/svelte'
    import ProjectChatInput from '$lib/components/project-chat-input.svelte'
    import { Badge } from '$lib/components/ui/badge/index.js'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Textarea } from '$lib/components/ui/textarea/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js'
    import * as Dialog from '$lib/components/ui/dialog/index.js'
    import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js'
    import { formatDateTime } from '$lib/utils/datetime'
    import type { TypeaheadResult } from '$lib/types/search.js'
    import type { PageData } from './$types.js'

    let { data }: { data: PageData } = $props()

    let instructions = $derived(data.project.instructions ?? '')
    let savingInstructions = $state(false)
    let showInstructionsDialog = $state(false)
    let showContextDialog = $state(false)
    let showDeleteConfirm = $state(false)

    let docQuery = $state('')
    let docResults = $state<TypeaheadResult[]>([])
    let searching = $state(false)
    let searchTimer: ReturnType<typeof setTimeout> | undefined

    $effect(() => {
        const query = docQuery.trim()
        if (searchTimer) clearTimeout(searchTimer)
        if (query.length < 2) {
            docResults = []
            searching = false
            return
        }
        searching = true
        searchTimer = setTimeout(async () => {
            try {
                const res = await fetch(`/api/typeahead?q=${encodeURIComponent(query)}&limit=8`)
                if (res.ok) {
                    const body: { results?: TypeaheadResult[] } = await res.json()
                    docResults = body.results ?? []
                }
            } finally {
                searching = false
            }
        }, 200)
    })

    async function saveInstructions() {
        savingInstructions = true
        try {
            const res = await fetch(`/api/projects/${data.project.id}`, {
                method: 'PATCH',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ instructions: instructions.trim() || null }),
            })
            if (res.ok) {
                showInstructionsDialog = false
                toast.success('Instructions saved')
                invalidateAll()
            } else {
                const body: { error?: string } = await res.json()
                toast.error(body.error || 'Failed to save instructions')
            }
        } catch {
            toast.error('Failed to save instructions')
        } finally {
            savingInstructions = false
        }
    }

    function cancelInstructions() {
        instructions = data.project.instructions ?? ''
        showInstructionsDialog = false
    }

    async function attachDocument(result: TypeaheadResult) {
        const res = await fetch(`/api/projects/${data.project.id}/attachments`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ attachmentType: 'document', documentId: result.document_id }),
        })
        if (res.ok) {
            const body: { alreadyAttached?: boolean } = await res.json()
            toast.success(body.alreadyAttached ? 'Already attached' : 'Document attached')
            docQuery = ''
            docResults = []
            invalidateAll()
        } else {
            const body: { error?: string } = await res.json()
            toast.error(body.error || 'Failed to attach document')
        }
    }

    async function removeAttachment(attachmentId: string) {
        const res = await fetch(`/api/projects/${data.project.id}/attachments`, {
            method: 'DELETE',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ attachmentId }),
        })
        if (res.ok) {
            toast.success('Attachment removed')
            invalidateAll()
        } else {
            const body: { error?: string } = await res.json()
            toast.error(body.error || 'Failed to remove attachment')
        }
    }

    async function toggleArchive() {
        const res = await fetch(`/api/projects/${data.project.id}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ isArchived: !data.project.isArchived }),
        })
        if (res.ok) {
            toast.success(data.project.isArchived ? 'Project unarchived' : 'Project archived')
            invalidateAll()
        } else {
            const body: { error?: string } = await res.json()
            toast.error(body.error || 'Failed to update project')
        }
    }

    async function handleDelete() {
        const res = await fetch(`/api/projects/${data.project.id}`, { method: 'DELETE' })
        if (res.ok) {
            toast.success('Project deleted. Its chats were kept and moved to unorganized.')
            await goto(resolve('/projects'))
        } else {
            const body: { error?: string } = await res.json()
            toast.error(body.error || 'Failed to delete project')
        }
    }
</script>

{#snippet projectContext(searchId: string)}
    <div class="space-y-7">
        <section class="border-t pt-6">
            <div class="flex items-center justify-between gap-3">
                <div>
                    <h3 class="text-sm font-semibold">Instructions</h3>
                    <p class="text-muted-foreground mt-1 text-xs">
                        Guide how Omni responds in every chat.
                    </p>
                </div>
                <Button
                    variant="ghost"
                    size="icon"
                    class="text-muted-foreground cursor-pointer"
                    aria-label="Edit project instructions"
                    disabled={data.project.isArchived}
                    onclick={() => (showInstructionsDialog = true)}>
                    <Pencil class="h-4 w-4" />
                </Button>
            </div>
            {#if instructions.trim()}
                <p class="text-muted-foreground mt-4 line-clamp-4 text-sm leading-6">
                    {instructions}
                </p>
            {:else}
                <p class="text-muted-foreground mt-4 text-sm italic">
                    Add instructions to guide every conversation.
                </p>
            {/if}
        </section>

        <section class="border-t pt-6">
            <div class="flex items-center justify-between gap-3">
                <div class="flex items-center gap-2">
                    <FileText class="h-4 w-4" />
                    <h3 class="text-sm font-semibold">Documents</h3>
                    <span class="text-muted-foreground text-xs">{data.attachments.length}</span>
                </div>
                <button
                    type="button"
                    class="text-muted-foreground hover:text-foreground cursor-pointer rounded-md p-1"
                    aria-label="Add documents"
                    onclick={() => document.getElementById(searchId)?.focus()}>
                    <Plus class="h-4 w-4" />
                </button>
            </div>
            <p class="text-muted-foreground mt-1 text-xs">
                Standing context for every conversation.
            </p>

            {#if data.attachments.length > 0}
                <ul class="mt-3 divide-y">
                    {#each data.attachments as attachment (attachment.id)}
                        <li class="flex items-center gap-2 py-2.5">
                            <span
                                class="bg-muted flex h-8 w-8 shrink-0 items-center justify-center rounded-md">
                                <FileText class="text-muted-foreground h-4 w-4" />
                            </span>
                            <span class="min-w-0 flex-1 truncate text-xs font-medium">
                                {attachment.title ?? 'Attachment'}
                            </span>
                            <button
                                type="button"
                                class="text-muted-foreground hover:text-foreground shrink-0 cursor-pointer p-1"
                                aria-label="Remove {attachment.title}"
                                disabled={data.project.isArchived}
                                onclick={() => removeAttachment(attachment.id)}>
                                <X class="h-3.5 w-3.5" />
                            </button>
                        </li>
                    {/each}
                </ul>
            {:else}
                <p class="text-muted-foreground mt-4 text-sm">No documents attached yet.</p>
            {/if}

            {#if !data.project.isArchived}
                <div class="mt-4 grid gap-2">
                    <Label for={searchId} class="text-muted-foreground text-xs font-normal"
                        >Add documents</Label>
                    <div class="relative">
                        <Search
                            class="text-muted-foreground pointer-events-none absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2" />
                        <Input
                            id={searchId}
                            bind:value={docQuery}
                            placeholder="Search documents..."
                            class="h-9 pl-9 text-xs" />
                    </div>
                    {#if docQuery.trim().length >= 2}
                        <div class="bg-background rounded-lg border">
                            {#if searching}
                                <p class="text-muted-foreground p-3 text-xs">Searching...</p>
                            {:else if docResults.length === 0}
                                <p class="text-muted-foreground p-3 text-xs">No documents found.</p>
                            {:else}
                                {#each docResults as result (result.document_id)}
                                    <button
                                        type="button"
                                        class="hover:bg-muted flex w-full cursor-pointer items-center justify-between gap-2 px-3 py-2 text-left text-xs"
                                        onclick={() => attachDocument(result)}>
                                        <span class="truncate">{result.title}</span>
                                        <Plus class="text-muted-foreground h-4 w-4 shrink-0" />
                                    </button>
                                {/each}
                            {/if}
                        </div>
                    {/if}
                </div>
            {/if}
        </section>
    </div>
{/snippet}

<svelte:head><title>{data.project.name} · Projects · Omni</title></svelte:head>

<div class="mx-auto w-full max-w-6xl px-4 py-6 sm:px-8 sm:py-10">
    <div class="mb-8 flex items-center justify-between gap-4">
        <a
            href={resolve('/projects')}
            class="text-muted-foreground hover:text-foreground hover:bg-muted inline-flex items-center gap-2 rounded-md px-2 py-1.5 text-sm leading-5 font-medium transition-colors">
            <ArrowLeft class="h-4 w-4 shrink-0" />
            <span>All projects</span>
        </a>
        <div class="flex items-center gap-2">
            <Dialog.Root bind:open={showContextDialog}>
                <Dialog.Trigger class="lg:hidden">
                    {#snippet child({ props })}
                        <Button variant="outline" class="cursor-pointer" {...props}>
                            <Layers class="mr-2 h-4 w-4" /> Context
                        </Button>
                    {/snippet}
                </Dialog.Trigger>
                <Dialog.Content class="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-lg">
                    {@render projectContext('mobile-doc-search')}
                </Dialog.Content>
            </Dialog.Root>
            <DropdownMenu.Root>
                <DropdownMenu.Trigger>
                    {#snippet child({ props })}
                        <Button variant="outline" size="icon" class="cursor-pointer" {...props}>
                            <Ellipsis class="h-4 w-4" />
                            <span class="sr-only">Project actions</span>
                        </Button>
                    {/snippet}
                </DropdownMenu.Trigger>
                <DropdownMenu.Content align="end">
                    <DropdownMenu.Item class="cursor-pointer" onclick={toggleArchive}>
                        {#if data.project.isArchived}
                            <ArchiveRestore class="mr-2 h-4 w-4" /> Unarchive
                        {:else}
                            <Archive class="mr-2 h-4 w-4" /> Archive
                        {/if}
                    </DropdownMenu.Item>
                    <DropdownMenu.Item
                        class="text-destructive cursor-pointer"
                        onclick={() => (showDeleteConfirm = true)}>
                        <Trash2 class="mr-2 h-4 w-4" /> Delete
                    </DropdownMenu.Item>
                </DropdownMenu.Content>
            </DropdownMenu.Root>
        </div>
    </div>

    <div class="mb-10 flex items-start gap-3 sm:gap-4">
        <div class="bg-muted shrink-0 rounded-xl p-2.5 sm:p-3">
            <Folder class="text-muted-foreground h-5 w-5 sm:h-6 sm:w-6" />
        </div>
        <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-3">
                <h1 class="text-2xl font-semibold tracking-tight break-words sm:text-3xl">
                    {data.project.name}
                </h1>
                {#if data.project.isArchived}<Badge variant="outline">Archived</Badge>{/if}
            </div>
            {#if data.project.description}
                <p class="text-muted-foreground mt-2 text-sm">{data.project.description}</p>
            {:else}
                <p class="text-muted-foreground mt-2 text-sm">
                    A shared home for conversations and context.
                </p>
            {/if}
        </div>
    </div>

    <div class="grid items-start gap-10 lg:grid-cols-[minmax(0,1fr)_17rem] lg:gap-14">
        <section class="min-w-0" aria-label="Project conversations">
            <h2 class="text-lg font-medium tracking-tight">Start a conversation</h2>
            <div class="mt-4">
                {#key data.project.id}
                    <ProjectChatInput
                        projectId={data.project.id}
                        models={data.models}
                        disabled={data.project.isArchived} />
                {/key}
            </div>
            <p class="text-muted-foreground mt-3 flex items-center gap-1.5 text-xs">
                <Layers class="h-3.5 w-3.5" />
                {data.project.isArchived
                    ? 'This project is archived. Unarchive it to start a new chat.'
                    : 'Your project context is included in every chat.'}
            </p>

            <div class="mt-10">
                <div class="mb-3 flex items-center gap-3">
                    <h2 class="text-sm font-semibold">Chats</h2>
                    <span class="text-muted-foreground text-xs">{data.chats.length}</span>
                    <div class="flex-1"></div>
                </div>
                {#if data.chats.length === 0}
                    <div class="rounded-xl border border-dashed px-6 py-10 text-center">
                        <MessageSquare class="text-muted-foreground mx-auto mb-3 h-6 w-6" />
                        <h3 class="text-sm font-medium">Start something here</h3>
                        <p class="text-muted-foreground mt-1 text-sm">
                            Send a message above and your project conversations will appear here.
                        </p>
                    </div>
                {:else}
                    <ul class="divide-y border-y">
                        {#each data.chats as chat (chat.id)}
                            <li>
                                <a
                                    href={resolve(`/chat/${chat.id}`)}
                                    class="hover:bg-muted/60 group flex items-center gap-3 rounded-lg px-2 py-4 transition-colors">
                                    <MessageSquare class="text-muted-foreground h-4 w-4 shrink-0" />
                                    <div class="min-w-0 flex-1">
                                        <p class="truncate text-sm font-medium">
                                            {chat.title ?? 'Untitled chat'}
                                        </p>
                                        <p class="text-muted-foreground mt-1 text-xs">
                                            {formatDateTime(
                                                chat.updatedAt,
                                                data.user.configuration?.timezone,
                                            )}
                                        </p>
                                    </div>
                                    <ChevronRight class="text-muted-foreground h-4 w-4 shrink-0" />
                                </a>
                            </li>
                        {/each}
                    </ul>
                {/if}
            </div>
        </section>

        <aside class="hidden min-w-0 border-l pl-6 lg:block" aria-label="Project context">
            {@render projectContext('desktop-doc-search')}
        </aside>
    </div>
</div>

<Dialog.Root bind:open={showInstructionsDialog}>
    <Dialog.Content class="sm:max-w-lg">
        <Dialog.Header>
            <Dialog.Title>Project instructions</Dialog.Title>
            <Dialog.Description>
                Set the direction once. Omni will use these instructions in every chat in {data
                    .project.name}.
            </Dialog.Description>
        </Dialog.Header>
        <div class="grid gap-2">
            <Label for="instructions-editor">Instructions</Label>
            <Textarea
                id="instructions-editor"
                bind:value={instructions}
                rows={7}
                autofocus
                placeholder="Include goals, tone, or details you don’t want to repeat."
                disabled={data.project.isArchived} />
            <p class="text-muted-foreground text-xs">
                Include goals, tone, or details you don’t want to repeat.
            </p>
        </div>
        <Dialog.Footer>
            <Button variant="outline" onclick={cancelInstructions} class="cursor-pointer"
                >Cancel</Button>
            <Button
                onclick={saveInstructions}
                disabled={savingInstructions || data.project.isArchived}
                class="cursor-pointer">
                <Save class="mr-2 h-4 w-4" />
                {savingInstructions ? 'Saving...' : 'Save instructions'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={showDeleteConfirm}>
    <AlertDialog.Content>
        <AlertDialog.Header>
            <AlertDialog.Title>Delete “{data.project.name}”?</AlertDialog.Title>
            <AlertDialog.Description>
                This permanently removes the project, its instructions, and its attachments. Chats
                in this project are kept and moved back to unorganized.
            </AlertDialog.Description>
        </AlertDialog.Header>
        <AlertDialog.Footer>
            <AlertDialog.Cancel class="cursor-pointer">Cancel</AlertDialog.Cancel>
            <AlertDialog.Action
                class="bg-destructive hover:bg-destructive/90 cursor-pointer text-white"
                onclick={handleDelete}>
                Delete project
            </AlertDialog.Action>
        </AlertDialog.Footer>
    </AlertDialog.Content>
</AlertDialog.Root>
