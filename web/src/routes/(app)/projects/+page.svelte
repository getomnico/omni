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
    import * as Tabs from '$lib/components/ui/tabs/index.js'
    import {
        Plus,
        Folder,
        Pencil,
        Trash2,
        Archive,
        ArchiveRestore,
        MoreVertical,
        Search,
        MessageSquare,
    } from '@lucide/svelte'
    import { invalidateAll } from '$app/navigation'
    import { resolve } from '$app/paths'
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

    type ProjectTab = 'active' | 'archived'
    let activeTab = $state<ProjectTab>('active')
    let showArchived = $derived(activeTab === 'archived')
    let query = $state('')

    let visibleProjects = $derived(
        data.projects.filter(
            (project) =>
                project.isArchived === showArchived &&
                `${project.name} ${project.description ?? ''}`
                    .toLowerCase()
                    .includes(query.trim().toLowerCase()),
        ),
    )
    let archivedCount = $derived(data.projects.filter((p) => p.isArchived).length)
    let activeCount = $derived(data.projects.filter((p) => !p.isArchived).length)

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

<svelte:head><title>Projects · Omni</title></svelte:head>

<div class="mx-auto w-full max-w-6xl px-4 py-6 sm:px-8 sm:py-10">
    <div class="mb-8 flex flex-wrap items-center justify-between gap-4">
        <div class="min-w-0 flex-1">
            <h1 class="text-2xl font-semibold tracking-tight sm:text-3xl">Projects</h1>
            <p class="text-muted-foreground mt-2 text-sm">
                Pick up where you left off, with everything in one place.
            </p>
        </div>
        <Button
            onclick={() => (showNewForm = true)}
            aria-label="New project"
            class="h-9 w-9 shrink-0 cursor-pointer p-0 sm:h-10 sm:w-auto sm:px-4">
            <Plus class="h-4 w-4 sm:mr-2" />
            <span class="hidden sm:inline">New project</span>
        </Button>
    </div>

    <Tabs.Root
        value={activeTab}
        onValueChange={(value) => (activeTab = value as ProjectTab)}
        class="w-full">
        <div
            class="mb-6 flex flex-col gap-2 border-b pb-2 sm:flex-row sm:items-center sm:justify-between sm:gap-4">
            <Tabs.List
                variant="line"
                class="order-2 gap-6 p-0 sm:order-1"
                aria-label="Project status">
                <Tabs.Trigger
                    value="active"
                    class="data-[state=active]:text-foreground data-[state=active]:after:bg-foreground text-muted-foreground h-11 cursor-pointer rounded-none px-1 data-[state=active]:font-semibold data-[state=active]:after:bottom-[-10px] data-[state=active]:after:opacity-100">
                    Active <span class="text-muted-foreground text-xs font-normal"
                        >{activeCount}</span>
                </Tabs.Trigger>
                <Tabs.Trigger
                    value="archived"
                    class="data-[state=active]:text-foreground data-[state=active]:after:bg-foreground text-muted-foreground h-11 cursor-pointer rounded-none px-1 data-[state=active]:font-semibold data-[state=active]:after:bottom-[-10px] data-[state=active]:after:opacity-100">
                    Archived <span class="text-muted-foreground text-xs font-normal"
                        >{archivedCount}</span>
                </Tabs.Trigger>
            </Tabs.List>
            <div
                class="bg-card order-1 flex h-10 w-full items-center gap-2 rounded-lg border px-3 pb-0 sm:order-2 sm:w-64 sm:pb-0">
                <Search class="text-muted-foreground h-4 w-4 shrink-0" />
                <Input
                    bind:value={query}
                    aria-label="Search projects"
                    placeholder="Search projects..."
                    class="h-9 min-w-0 flex-1 border-0 bg-transparent px-0 shadow-none focus-visible:ring-0" />
            </div>
        </div>

        <Tabs.Content value={activeTab} class="mt-0">
            {#if visibleProjects.length === 0}
                <div
                    class="flex flex-col items-center justify-center rounded-lg border border-dashed p-12 text-center">
                    <Folder class="text-muted-foreground mb-4 h-12 w-12" />
                    <h2 class="mb-2 text-lg font-medium">
                        {query.trim()
                            ? 'No matching projects'
                            : showArchived
                              ? 'No archived projects'
                              : 'Give your work a home'}
                    </h2>
                    <p class="text-muted-foreground mb-4 max-w-sm text-sm">
                        {query.trim()
                            ? 'Try a different name or description.'
                            : showArchived
                              ? 'Projects you archive will appear here.'
                              : 'Bring related chats, instructions, and key documents together in your first project.'}
                    </p>
                    {#if query.trim()}
                        <Button
                            variant="outline"
                            onclick={() => (query = '')}
                            class="cursor-pointer">Clear search</Button>
                    {:else if !showArchived}
                        <Button onclick={() => (showNewForm = true)} class="cursor-pointer">
                            <Plus class="mr-2 h-4 w-4" /> Create project
                        </Button>
                    {/if}
                </div>
            {:else}
                <div class="grid gap-5 md:grid-cols-2">
                    {#each visibleProjects as project (project.id)}
                        <Card.Root
                            class="group hover:border-primary/30 relative flex min-h-[214px] gap-0 shadow-none transition-all hover:shadow-sm sm:min-h-[214px]">
                            <Card.Header class="flex-1 pb-4">
                                <div class="flex items-start justify-between gap-2">
                                    <div class="bg-muted text-muted-foreground rounded-lg p-2.5">
                                        <Folder class="h-5 w-5" />
                                    </div>
                                    <DropdownMenu.Root>
                                        <DropdownMenu.Trigger>
                                            {#snippet child({ props })}
                                                <Button
                                                    variant="ghost"
                                                    size="icon"
                                                    class="relative z-10 cursor-pointer"
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
                                <Card.Title class="mt-4 text-lg leading-snug">
                                    <a
                                        href={resolve(`/projects/${project.id}`)}
                                        class="focus-visible:after:ring-ring after:absolute after:inset-0 after:rounded-xl focus-visible:outline-none focus-visible:after:ring-2">
                                        <span class="break-words">{project.name}</span>
                                    </a>
                                </Card.Title>
                                <Card.Description class="mt-2 line-clamp-2 min-h-10 leading-5">
                                    {project.description ??
                                        'Add instructions and documents to give your chats shared context.'}
                                </Card.Description>
                            </Card.Header>
                            <Card.Footer
                                class="text-muted-foreground mt-auto flex flex-wrap items-center gap-x-4 gap-y-2 pt-3 text-xs">
                                <span class="inline-flex items-center gap-1.5">
                                    <MessageSquare class="h-3.5 w-3.5" />
                                    {project.chatCount}
                                    {project.chatCount === 1 ? 'chat' : 'chats'}
                                </span>
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
        </Tabs.Content>
    </Tabs.Root>
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
