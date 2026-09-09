<script lang="ts">
    import { Button } from '$lib/components/ui/button/index.js'
    import { Badge } from '$lib/components/ui/badge/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import { Textarea } from '$lib/components/ui/textarea/index.js'
    import * as Card from '$lib/components/ui/card/index.js'
    import * as Dialog from '$lib/components/ui/dialog/index.js'
    import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js'
    import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js'
    import {
        Plus,
        Folder,
        Pencil,
        Trash2,
        Archive,
        ArchiveRestore,
        MoreVertical,
    } from '@lucide/svelte'
    import { invalidateAll } from '$app/navigation'
    import { toast } from 'svelte-sonner'
    import { formatDateTime } from '$lib/utils/datetime'
    import type { PageData } from './$types.js'
    import type { Project } from '$lib/server/db/schema.js'

    let { data }: { data: PageData } = $props()

    let showNewForm = $state(false)
    let newName = $state('')
    let newDescription = $state('')
    let newInstructions = $state('')
    let saving = $state(false)

    let showEditForm = $state(false)
    let editingProject = $state<Project | null>(null)
    let editName = $state('')
    let editDescription = $state('')
    let editInstructions = $state('')

    let showDeleteConfirm = $state(false)
    let deletingProject = $state<Project | null>(null)

    let showArchived = $state(false)

    let visibleProjects = $derived(
        data.projects.filter((project) => showArchived || !project.isArchived),
    )
    let archivedCount = $derived(data.projects.filter((p) => p.isArchived).length)

    function resetNewForm() {
        newName = ''
        newDescription = ''
        newInstructions = ''
    }

    function openEdit(project: Project) {
        editingProject = project
        editName = project.name
        editDescription = project.description ?? ''
        editInstructions = project.instructions ?? ''
        showEditForm = true
    }

    async function handleCreate() {
        if (!newName.trim()) {
            toast.error('Name is required')
            return
        }
        saving = true
        try {
            const res = await fetch('/api/projects', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: newName.trim(),
                    description: newDescription.trim() || null,
                    instructions: newInstructions.trim() || null,
                }),
            })
            if (res.ok) {
                showNewForm = false
                resetNewForm()
                toast.success('Project created')
                invalidateAll()
            } else {
                const body = await res.json()
                toast.error(body.error || 'Failed to create project')
            }
        } catch {
            toast.error('Failed to create project')
        } finally {
            saving = false
        }
    }

    async function handleUpdate() {
        if (!editingProject) return
        if (!editName.trim()) {
            toast.error('Name is required')
            return
        }
        saving = true
        try {
            const res = await fetch(`/api/projects/${editingProject.id}`, {
                method: 'PATCH',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: editName.trim(),
                    description: editDescription.trim() || null,
                    instructions: editInstructions.trim() || null,
                }),
            })
            if (res.ok) {
                showEditForm = false
                editingProject = null
                toast.success('Project updated')
                invalidateAll()
            } else {
                const body = await res.json()
                toast.error(body.error || 'Failed to update project')
            }
        } catch {
            toast.error('Failed to update project')
        } finally {
            saving = false
        }
    }

    async function toggleArchive(project: Project) {
        const res = await fetch(`/api/projects/${project.id}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ isArchived: !project.isArchived }),
        })
        if (res.ok) {
            toast.success(project.isArchived ? 'Project unarchived' : 'Project archived')
            invalidateAll()
        } else {
            const body = await res.json()
            toast.error(body.error || 'Failed to update project')
        }
    }

    async function handleDelete() {
        if (!deletingProject) return
        try {
            const res = await fetch(`/api/projects/${deletingProject.id}`, {
                method: 'DELETE',
            })
            if (res.ok) {
                toast.success('Project deleted. Its chats were kept and moved to unorganized.')
                invalidateAll()
            } else {
                const body = await res.json()
                toast.error(body.error || 'Failed to delete project')
            }
        } catch {
            toast.error('Failed to delete project')
        } finally {
            showDeleteConfirm = false
            deletingProject = null
        }
    }
</script>

<div class="mx-auto max-w-4xl p-6">
    <div class="mb-6 flex items-center justify-between">
        <div>
            <h1 class="text-2xl font-bold">Projects</h1>
            <p class="text-muted-foreground text-sm">
                Organize related chats, instructions, and context in one place
            </p>
        </div>
        <Button onclick={() => (showNewForm = true)} class="cursor-pointer">
            <Plus class="mr-2 h-4 w-4" />
            New Project
        </Button>
    </div>

    {#if data.projects.length > 0}
        <label class="text-muted-foreground mb-4 flex cursor-pointer items-center gap-2 text-sm">
            <input
                type="checkbox"
                bind:checked={showArchived}
                class="accent-primary h-4 w-4 cursor-pointer" />
            Show archived ({archivedCount})
        </label>
    {/if}

    {#if visibleProjects.length === 0}
        <div
            class="flex flex-col items-center justify-center rounded-lg border border-dashed p-12 text-center">
            <Folder class="text-muted-foreground mb-4 h-12 w-12" />
            <h3 class="mb-2 text-lg font-medium">No projects yet</h3>
            <p class="text-muted-foreground mb-4 text-sm">
                Create a project to group chats, share standing instructions, and keep key documents
                at hand.
            </p>
            <Button onclick={() => (showNewForm = true)} class="cursor-pointer">
                <Plus class="mr-2 h-4 w-4" />
                Create your first project
            </Button>
        </div>
    {:else}
        <div class="grid gap-4 sm:grid-cols-2">
            {#each visibleProjects as project (project.id)}
                <Card.Root>
                    <Card.Header>
                        <div class="flex items-start justify-between gap-2">
                            <a href={`/projects/${project.id}`} class="hover:underline">
                                <Card.Title class="flex items-center gap-2">
                                    <Folder class="text-muted-foreground h-4 w-4" />
                                    {project.name}
                                </Card.Title>
                            </a>
                            <DropdownMenu.Root>
                                <DropdownMenu.Trigger>
                                    {#snippet child({ props })}
                                        <Button
                                            variant="ghost"
                                            size="icon"
                                            class="cursor-pointer"
                                            {...props}>
                                            <MoreVertical class="h-4 w-4" />
                                            <span class="sr-only">Project actions</span>
                                        </Button>
                                    {/snippet}
                                </DropdownMenu.Trigger>
                                <DropdownMenu.Content align="end">
                                    <DropdownMenu.Item
                                        class="cursor-pointer"
                                        onclick={() => openEdit(project)}>
                                        <Pencil class="mr-2 h-4 w-4" />
                                        Edit
                                    </DropdownMenu.Item>
                                    <DropdownMenu.Item
                                        class="cursor-pointer"
                                        onclick={() => toggleArchive(project)}>
                                        {#if project.isArchived}
                                            <ArchiveRestore class="mr-2 h-4 w-4" />
                                            Unarchive
                                        {:else}
                                            <Archive class="mr-2 h-4 w-4" />
                                            Archive
                                        {/if}
                                    </DropdownMenu.Item>
                                    <DropdownMenu.Item
                                        class="text-destructive cursor-pointer"
                                        onclick={() => {
                                            deletingProject = project
                                            showDeleteConfirm = true
                                        }}>
                                        <Trash2 class="mr-2 h-4 w-4" />
                                        Delete
                                    </DropdownMenu.Item>
                                </DropdownMenu.Content>
                            </DropdownMenu.Root>
                        </div>
                        <Card.Description>
                            {project.description || 'No description'}
                        </Card.Description>
                    </Card.Header>
                    <Card.Content>
                        <a href={`/projects/${project.id}`} class="text-sm hover:underline">
                            View project
                        </a>
                    </Card.Content>
                    <Card.Footer class="flex items-center justify-between text-xs">
                        <Badge variant="secondary">
                            {project.chatCount}
                            {project.chatCount === 1 ? 'chat' : 'chats'}
                        </Badge>
                        {#if project.isArchived}
                            <Badge variant="outline">Archived</Badge>
                        {/if}
                        <span class="text-muted-foreground">
                            Updated {formatDateTime(
                                project.updatedAt,
                                data.user.configuration?.timezone,
                            )}
                        </span>
                    </Card.Footer>
                </Card.Root>
            {/each}
        </div>
    {/if}
</div>

<Dialog.Root bind:open={showNewForm}>
    <Dialog.Content class="sm:max-w-lg">
        <Dialog.Header>
            <Dialog.Title>New project</Dialog.Title>
            <Dialog.Description>
                Group chats, standing instructions, and context documents.
            </Dialog.Description>
        </Dialog.Header>
        <div class="grid gap-4 py-4">
            <div class="grid gap-2">
                <Label for="project-name">Name</Label>
                <Input id="project-name" bind:value={newName} placeholder="Q3 Launch" />
            </div>
            <div class="grid gap-2">
                <Label for="project-description">Description (optional)</Label>
                <Input
                    id="project-description"
                    bind:value={newDescription}
                    placeholder="What is this project about?" />
            </div>
            <div class="grid gap-2">
                <Label for="project-instructions">Instructions (optional)</Label>
                <Textarea
                    id="project-instructions"
                    bind:value={newInstructions}
                    placeholder="Standing instructions applied to every chat in this project"
                    rows={4} />
            </div>
        </div>
        <Dialog.Footer>
            <Button variant="outline" onclick={() => (showNewForm = false)} class="cursor-pointer">
                Cancel
            </Button>
            <Button onclick={handleCreate} disabled={saving} class="cursor-pointer">
                {saving ? 'Creating...' : 'Create project'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>

<Dialog.Root bind:open={showEditForm}>
    <Dialog.Content class="sm:max-w-lg">
        <Dialog.Header>
            <Dialog.Title>Edit project</Dialog.Title>
        </Dialog.Header>
        <div class="grid gap-4 py-4">
            <div class="grid gap-2">
                <Label for="edit-project-name">Name</Label>
                <Input id="edit-project-name" bind:value={editName} />
            </div>
            <div class="grid gap-2">
                <Label for="edit-project-description">Description</Label>
                <Input id="edit-project-description" bind:value={editDescription} />
            </div>
            <div class="grid gap-2">
                <Label for="edit-project-instructions">Instructions</Label>
                <Textarea
                    id="edit-project-instructions"
                    bind:value={editInstructions}
                    placeholder="Standing instructions applied to every chat in this project"
                    rows={4} />
            </div>
        </div>
        <Dialog.Footer>
            <Button variant="outline" onclick={() => (showEditForm = false)} class="cursor-pointer">
                Cancel
            </Button>
            <Button onclick={handleUpdate} disabled={saving} class="cursor-pointer">
                {saving ? 'Saving...' : 'Save changes'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={showDeleteConfirm}>
    <AlertDialog.Content>
        <AlertDialog.Header>
            <AlertDialog.Title>Delete "{deletingProject?.name}"?</AlertDialog.Title>
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
