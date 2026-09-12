<script lang="ts">
    import { Button } from '$lib/components/ui/button/index.js'
    import { Badge } from '$lib/components/ui/badge/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import { Textarea } from '$lib/components/ui/textarea/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import * as Card from '$lib/components/ui/card/index.js'
    import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js'
    import {
        MessageSquare,
        Save,
        Trash2,
        Plus,
        Search,
        X,
        Archive,
        ArchiveRestore,
    } from '@lucide/svelte'
    import { goto, invalidateAll } from '$app/navigation'
    import { toast } from 'svelte-sonner'
    import { resolvePreferredModelId } from '$lib/preferences'
    import { formatDateTime } from '$lib/utils/datetime'
    import type { PageData } from './$types.js'
    import type { TypeaheadResult } from '$lib/types/search.js'

    let { data }: { data: PageData } = $props()

    let instructions = $state(data.project.instructions ?? '')
    let savingInstructions = $state(false)
    let instructionsDirty = $derived(instructions !== (data.project.instructions ?? ''))

    let showDeleteConfirm = $state(false)

    let addingChat = $state(false)

    // Document attachment search (reuses the permission-enforced typeahead).
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
                    const body = await res.json()
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
                toast.success('Instructions saved')
                invalidateAll()
            } else {
                const body = await res.json()
                toast.error(body.error || 'Failed to save instructions')
            }
        } catch {
            toast.error('Failed to save instructions')
        } finally {
            savingInstructions = false
        }
    }

    async function attachDocument(result: TypeaheadResult) {
        const res = await fetch(`/api/projects/${data.project.id}/attachments`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                attachmentType: 'document',
                documentId: result.document_id,
            }),
        })
        if (res.ok) {
            const body = await res.json()
            toast.success(body.alreadyAttached ? 'Already attached' : 'Document attached')
            docQuery = ''
            docResults = []
            invalidateAll()
        } else {
            const body = await res.json()
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
            const body = await res.json()
            toast.error(body.error || 'Failed to remove attachment')
        }
    }

    async function newChat() {
        addingChat = true
        try {
            const modelId = resolvePreferredModelId(data.models)
            const res = await fetch('/api/chat', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ projectId: data.project.id, modelId }),
            })
            if (res.ok) {
                const { chatId } = await res.json()
                // No message has been sent yet, so there is no run to stream.
                // Requesting a stream on an empty regular chat fails with 404.
                await goto(`/chat/${chatId}`)
            } else {
                const body = await res.json()
                toast.error(body.error || 'Failed to create chat')
            }
        } catch {
            toast.error('Failed to create chat')
        } finally {
            addingChat = false
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
            const body = await res.json()
            toast.error(body.error || 'Failed to update project')
        }
    }

    async function handleDelete() {
        const res = await fetch(`/api/projects/${data.project.id}`, { method: 'DELETE' })
        if (res.ok) {
            toast.success('Project deleted. Its chats were kept and moved to unorganized.')
            await goto('/projects')
        } else {
            const body = await res.json()
            toast.error(body.error || 'Failed to delete project')
        }
    }
</script>

<div class="mx-auto max-w-4xl p-6">
    <div class="mb-6 flex items-start justify-between gap-4">
        <div>
            <div class="flex items-center gap-3">
                <h1 class="text-2xl font-bold">{data.project.name}</h1>
                {#if data.project.isArchived}
                    <Badge variant="outline">Archived</Badge>
                {/if}
            </div>
            {#if data.project.description}
                <p class="text-muted-foreground mt-1 text-sm">{data.project.description}</p>
            {/if}
        </div>
        <div class="flex shrink-0 items-center gap-2">
            <Button onclick={newChat} disabled={addingChat} class="cursor-pointer">
                <MessageSquare class="mr-2 h-4 w-4" />
                {addingChat ? 'Starting...' : 'New chat'}
            </Button>
            <Button variant="outline" onclick={toggleArchive} class="cursor-pointer">
                {#if data.project.isArchived}
                    <ArchiveRestore class="mr-2 h-4 w-4" />
                    Unarchive
                {:else}
                    <Archive class="mr-2 h-4 w-4" />
                    Archive
                {/if}
            </Button>
            <Button
                variant="outline"
                class="text-destructive hover:text-destructive cursor-pointer"
                onclick={() => (showDeleteConfirm = true)}>
                <Trash2 class="mr-2 h-4 w-4" />
                Delete
            </Button>
        </div>
    </div>

    <div class="grid gap-6">
        <!-- Instructions -->
        <Card.Root>
            <Card.Header>
                <Card.Title>Project instructions</Card.Title>
                <Card.Description>
                    Automatically included in every chat in this project.
                </Card.Description>
            </Card.Header>
            <Card.Content class="grid gap-3">
                <Textarea
                    bind:value={instructions}
                    rows={5}
                    placeholder="e.g. This project is about the Q3 launch. Prefer German responses and reference the roadmap."
                    disabled={data.project.isArchived} />
            </Card.Content>
            <Card.Footer class="justify-end">
                <Button
                    onclick={saveInstructions}
                    disabled={!instructionsDirty || savingInstructions || data.project.isArchived}
                    class="cursor-pointer">
                    <Save class="mr-2 h-4 w-4" />
                    {savingInstructions ? 'Saving...' : 'Save instructions'}
                </Button>
            </Card.Footer>
        </Card.Root>

        <!-- Attachments -->
        <Card.Root>
            <Card.Header>
                <Card.Title>Context documents</Card.Title>
                <Card.Description>
                    Attached documents are available as standing context in every chat in this
                    project.
                </Card.Description>
            </Card.Header>
            <Card.Content class="grid gap-4">
                {#if data.attachments.length > 0}
                    <div class="flex flex-wrap gap-2">
                        {#each data.attachments as attachment (attachment.id)}
                            <Badge
                                variant="secondary"
                                class="flex max-w-full items-center gap-1 py-1 pl-2">
                                <span class="truncate">{attachment.title ?? 'Attachment'}</span>
                                <button
                                    type="button"
                                    class="hover:text-destructive ml-1 cursor-pointer"
                                    aria-label="Remove {attachment.title}"
                                    onclick={() => removeAttachment(attachment.id)}>
                                    <X class="h-3 w-3" />
                                </button>
                            </Badge>
                        {/each}
                    </div>
                {:else}
                    <p class="text-muted-foreground text-sm">No documents attached yet.</p>
                {/if}

                {#if !data.project.isArchived}
                    <div class="grid gap-2">
                        <Label for="doc-search">Add documents</Label>
                        <div class="relative">
                            <Search
                                class="text-muted-foreground pointer-events-none absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2" />
                            <Input
                                id="doc-search"
                                bind:value={docQuery}
                                placeholder="Search indexed documents to attach..."
                                class="pl-9" />
                        </div>
                        {#if docQuery.trim().length >= 2}
                            <div class="rounded-md border">
                                {#if searching}
                                    <p class="text-muted-foreground p-3 text-sm">Searching...</p>
                                {:else if docResults.length === 0}
                                    <p class="text-muted-foreground p-3 text-sm">
                                        No documents found. Try a different search.
                                    </p>
                                {:else}
                                    {#each docResults as result (result.document_id)}
                                        <button
                                            type="button"
                                            class="hover:bg-accent flex w-full cursor-pointer items-center justify-between gap-2 px-3 py-2 text-left text-sm"
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
            </Card.Content>
        </Card.Root>

        <!-- Chats -->
        <Card.Root>
            <Card.Header>
                <Card.Title>Chats</Card.Title>
                <Card.Description>Conversations in this project.</Card.Description>
            </Card.Header>
            <Card.Content>
                {#if data.chats.length === 0}
                    <p class="text-muted-foreground text-sm">No chats yet.</p>
                {:else}
                    <ul class="divide-y">
                        {#each data.chats as chat (chat.id)}
                            <li>
                                <a
                                    href={`/chat/${chat.id}`}
                                    class="hover:bg-accent flex items-center justify-between gap-4 px-2 py-3">
                                    <span class="truncate text-sm">
                                        {chat.title ?? 'Untitled chat'}
                                    </span>
                                    <span class="text-muted-foreground shrink-0 text-xs">
                                        {formatDateTime(
                                            chat.updatedAt,
                                            data.user.configuration?.timezone,
                                        )}
                                    </span>
                                </a>
                            </li>
                        {/each}
                    </ul>
                {/if}
            </Card.Content>
        </Card.Root>
    </div>
</div>

<AlertDialog.Root bind:open={showDeleteConfirm}>
    <AlertDialog.Content>
        <AlertDialog.Header>
            <AlertDialog.Title>Delete "{data.project.name}"?</AlertDialog.Title>
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
