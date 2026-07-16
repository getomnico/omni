<script lang="ts">
    import { Button } from '$lib/components/ui/button/index.js'
    import { Badge } from '$lib/components/ui/badge/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import { Switch } from '$lib/components/ui/switch/index.js'
    import * as Tabs from '$lib/components/ui/tabs/index.js'
    import * as Card from '$lib/components/ui/card/index.js'
    import * as Dialog from '$lib/components/ui/dialog/index.js'
    import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js'
    import { Tooltip, TooltipTrigger, TooltipContent } from '$lib/components/ui/tooltip/index.js'
    import { Plus, BookOpen, Copy, Pencil, Trash2, Globe, Lock, Search } from '@lucide/svelte'
    import { invalidateAll } from '$app/navigation'
    import { toast } from 'svelte-sonner'
    import { formatDateTime } from '$lib/utils/datetime'
    import type { PageData } from './$types.js'
    import type { Skill } from '$lib/server/db/schema.js'

    let { data }: { data: PageData } = $props()

    let tab = $state('mine')

    let showNewForm = $state(false)
    let newName = $state('')
    let newInstructions = $state('')
    let newIsPublic = $state(false)
    let saving = $state(false)
    let filterQuery = $state('')

    let showEditForm = $state(false)
    let editingSkill = $state<Skill | null>(null)
    let editName = $state('')
    let editInstructions = $state('')
    let editIsPublic = $state(false)
    let showDeleteConfirm = $state(false)
    let deletingSkill = $state<Skill | null>(null)

    let cloningIds = $state<string[]>([])

    function matchesFilter(skill: Skill) {
        const query = filterQuery.trim().toLocaleLowerCase()
        if (!query) return true
        return (
            skill.name.toLocaleLowerCase().includes(query) ||
            skill.instructions.toLocaleLowerCase().includes(query) ||
            `library:${skill.id}`.toLocaleLowerCase().includes(query)
        )
    }

    let mySkills = $derived(
        data.skills.filter((skill) => skill.ownerId === data.user.id && matchesFilter(skill)),
    )
    let publicSkills = $derived(
        data.skills.filter(
            (skill) =>
                skill.visibility === 'public' &&
                skill.ownerId !== data.user.id &&
                matchesFilter(skill),
        ),
    )

    function resetNewForm() {
        newName = ''
        newInstructions = ''
        newIsPublic = false
    }

    function openEdit(skill: Skill) {
        editingSkill = skill
        editName = skill.name
        editInstructions = skill.instructions
        editIsPublic = skill.visibility === 'public'
        showEditForm = true
    }

    function openDelete(skill: Skill) {
        deletingSkill = skill
        showDeleteConfirm = true
    }

    async function handleCreate() {
        if (!newName.trim() || !newInstructions.trim()) {
            toast.error('Name and instructions are required')
            return
        }
        saving = true
        try {
            const res = await fetch('/api/skills', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: newName.trim(),
                    instructions: newInstructions.trim(),
                    visibility: newIsPublic ? 'public' : 'private',
                }),
            })
            if (res.ok) {
                showNewForm = false
                toast.success('Skill created')
                invalidateAll()
            } else {
                const body = await res.json()
                toast.error(body.error || 'Failed to create skill')
            }
        } catch {
            toast.error('Failed to create skill')
        } finally {
            saving = false
        }
    }

    async function handleUpdate() {
        if (!editingSkill) return
        if (!editName.trim() || !editInstructions.trim()) {
            toast.error('Name and instructions are required')
            return
        }
        saving = true
        try {
            const res = await fetch(`/api/skills/${editingSkill.id}`, {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: editName.trim(),
                    instructions: editInstructions.trim(),
                    visibility: editIsPublic ? 'public' : 'private',
                }),
            })
            if (res.ok) {
                showEditForm = false
                editingSkill = null
                toast.success('Skill updated')
                invalidateAll()
            } else {
                const body = await res.json()
                toast.error(body.error || 'Failed to update skill')
            }
        } catch {
            toast.error('Failed to update skill')
        } finally {
            saving = false
        }
    }

    async function confirmDelete() {
        if (!deletingSkill) return
        const skillId = deletingSkill.id
        deletingSkill = null
        showDeleteConfirm = false

        try {
            const res = await fetch(`/api/skills/${skillId}`, { method: 'DELETE' })
            if (!res.ok) {
                const body = await res.json()
                toast.error(body.error || 'Failed to delete skill')
                return
            }
            toast.success('Skill deleted')
            invalidateAll()
        } catch {
            toast.error('Failed to delete skill')
        }
    }

    async function handleClone(skillId: string) {
        cloningIds = [...cloningIds, skillId]
        try {
            const res = await fetch(`/api/skills/${skillId}/clone`, { method: 'POST' })
            if (res.ok) {
                toast.success('Skill cloned as a private copy')
                await invalidateAll()
                tab = 'mine'
                return
            }
            const body = await res.json()
            toast.error(body.error || 'Failed to clone skill')
        } catch {
            toast.error('Failed to clone skill')
        } finally {
            cloningIds = cloningIds.filter((id) => id !== skillId)
        }
    }
</script>

<div class="mx-auto max-w-4xl p-6">
    <div class="mb-6 flex items-center justify-between">
        <div>
            <h1 class="text-2xl font-bold">Skill Library</h1>
            <p class="text-muted-foreground text-sm">
                Create and manage workplace skills. Public skills are visible to everyone.
            </p>
        </div>
        <Button
            class="cursor-pointer"
            onclick={() => {
                resetNewForm()
                showNewForm = true
            }}>
            <Plus class="mr-2 h-4 w-4" />
            New Skill
        </Button>
    </div>

    <!-- Create skill dialog -->
    <Dialog.Root bind:open={showNewForm}>
        <Dialog.Content
            class="flex h-[calc(100dvh-2rem)] max-h-[calc(100dvh-2rem)] flex-col sm:max-w-3xl lg:max-w-4xl">
            <Dialog.Header>
                <Dialog.Title>Create Skill</Dialog.Title>
                <Dialog.Description>
                    Write instructions that the AI will use when this skill is loaded.
                </Dialog.Description>
            </Dialog.Header>
            <form
                onsubmit={(e) => {
                    e.preventDefault()
                    handleCreate()
                }}
                class="flex min-h-0 flex-1 flex-col gap-4">
                <div class="space-y-2">
                    <Label for="new-name">Name</Label>
                    <Input
                        id="new-name"
                        bind:value={newName}
                        placeholder="e.g., PR Review Checklist" />
                </div>
                <div class="flex min-h-0 flex-1 flex-col space-y-2">
                    <Label for="new-instructions">Instructions</Label>
                    <div
                        id="new-instructions"
                        bind:innerText={newInstructions}
                        class="before:text-muted-foreground border-input focus-visible:border-ring focus-visible:ring-ring/50 relative min-h-0 flex-1 cursor-text overflow-y-auto rounded-md border bg-transparent px-3 py-2 text-base break-words whitespace-pre-wrap shadow-xs transition-[color,box-shadow] outline-none focus-visible:ring-[3px] md:text-sm"
                        class:before:content-[attr(data-placeholder)]={!newInstructions.trim()}
                        class:before:content-none={newInstructions.trim()}
                        contenteditable="plaintext-only"
                        role="textbox"
                        aria-multiline="true"
                        data-placeholder="Describe the task, what tools to use, and how to format the response...">
                    </div>
                </div>
                <div class="flex items-center gap-2">
                    <Switch
                        id="new-is-public"
                        checked={newIsPublic}
                        onCheckedChange={(v) => (newIsPublic = v)}
                        class="cursor-pointer" />
                    <Label for="new-is-public" class="cursor-pointer">Public</Label>
                </div>
                <Dialog.Footer>
                    <Button
                        variant="outline"
                        class="cursor-pointer"
                        onclick={() => {
                            showNewForm = false
                        }}
                        type="button">Cancel</Button>
                    <Button type="submit" disabled={saving} class="cursor-pointer"
                        >{saving ? 'Creating...' : 'Create'}</Button>
                </Dialog.Footer>
            </form>
        </Dialog.Content>
    </Dialog.Root>

    <!-- Edit skill dialog -->
    <Dialog.Root bind:open={showEditForm}>
        <Dialog.Content
            class="flex h-[calc(100dvh-2rem)] max-h-[calc(100dvh-2rem)] flex-col sm:max-w-3xl lg:max-w-4xl">
            <Dialog.Header>
                <Dialog.Title>Edit Skill</Dialog.Title>
                <Dialog.Description
                    >Update the skill name, instructions, or visibility.</Dialog.Description>
            </Dialog.Header>
            <form
                onsubmit={(e) => {
                    e.preventDefault()
                    handleUpdate()
                }}
                class="flex min-h-0 flex-1 flex-col gap-4">
                <div class="space-y-2">
                    <Label for="edit-name">Name</Label>
                    <Input id="edit-name" bind:value={editName} />
                </div>
                <div class="flex min-h-0 flex-1 flex-col space-y-2">
                    <Label for="edit-instructions">Instructions</Label>
                    <div
                        id="edit-instructions"
                        bind:innerText={editInstructions}
                        class="before:text-muted-foreground border-input focus-visible:border-ring focus-visible:ring-ring/50 relative min-h-0 flex-1 cursor-text overflow-y-auto rounded-md border bg-transparent px-3 py-2 text-base break-words whitespace-pre-wrap shadow-xs transition-[color,box-shadow] outline-none focus-visible:ring-[3px] md:text-sm"
                        class:before:content-[attr(data-placeholder)]={!editInstructions.trim()}
                        class:before:content-none={editInstructions.trim()}
                        contenteditable="plaintext-only"
                        role="textbox"
                        aria-multiline="true"
                        data-placeholder="Describe the task, what tools to use, and how to format the response...">
                    </div>
                </div>
                <div class="flex items-center gap-2">
                    <Switch
                        id="edit-is-public"
                        checked={editIsPublic}
                        onCheckedChange={(v) => (editIsPublic = v)}
                        class="cursor-pointer" />
                    <Label for="edit-is-public" class="cursor-pointer">Public</Label>
                </div>
                <Dialog.Footer>
                    <Button
                        variant="outline"
                        class="cursor-pointer"
                        onclick={() => {
                            showEditForm = false
                        }}
                        type="button">Cancel</Button>
                    <Button type="submit" disabled={saving} class="cursor-pointer"
                        >{saving ? 'Saving...' : 'Save Changes'}</Button>
                </Dialog.Footer>
            </form>
        </Dialog.Content>
    </Dialog.Root>

    <div class="mb-4">
        <Label for="skill-filter">Filter skills</Label>
        <div class="relative mt-2">
            <Search
                class="text-muted-foreground pointer-events-none absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2" />
            <Input
                id="skill-filter"
                bind:value={filterQuery}
                placeholder="Search by name, instructions, or library:<id>"
                class="pl-9" />
        </div>
    </div>

    <!-- Tabs -->
    <Tabs.Root value={tab} onValueChange={(v) => (tab = v)} class="w-full">
        <Tabs.List class="mb-4">
            <Tabs.Trigger
                value="mine"
                class="data-[state=active]:bg-background data-[state=active]:text-foreground cursor-pointer data-[state=active]:shadow-sm">
                My Skills ({mySkills.length})
            </Tabs.Trigger>
            <Tabs.Trigger
                value="public"
                class="data-[state=active]:bg-background data-[state=active]:text-foreground cursor-pointer data-[state=active]:shadow-sm">
                Public Library ({publicSkills.length})
            </Tabs.Trigger>
        </Tabs.List>

        <!-- My Skills tab -->
        <Tabs.Content value="mine">
            {#if mySkills.length === 0}
                <div
                    class="flex flex-col items-center justify-center rounded-lg border border-dashed p-12 text-center">
                    <BookOpen class="text-muted-foreground mb-4 h-12 w-12" />
                    <h3 class="mb-2 text-lg font-medium">No skills yet</h3>
                    <p class="text-muted-foreground mb-4 text-sm">
                        Create a skill with reusable instructions for common tasks.
                    </p>
                    <Button
                        class="cursor-pointer"
                        onclick={() => {
                            resetNewForm()
                            showNewForm = true
                        }}>
                        <Plus class="mr-2 h-4 w-4" />
                        Create your first skill
                    </Button>
                </div>
            {:else}
                <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
                    {#each mySkills as skill (skill.id)}
                        <Card.Root class="group hover:bg-muted/50 transition-colors">
                            <Card.Content class="flex items-start justify-between">
                                <div class="min-w-0 flex-1">
                                    <div class="flex items-center gap-2">
                                        <h3 class="font-medium">{skill.name}</h3>
                                        {#if skill.visibility === 'public'}
                                            <Badge variant="outline">
                                                <Globe class="mr-1 h-3 w-3" />
                                                Public
                                            </Badge>
                                        {:else}
                                            <Badge variant="secondary">
                                                <Lock class="mr-1 h-3 w-3" />
                                                Private
                                            </Badge>
                                        {/if}
                                    </div>
                                    <p class="text-muted-foreground mt-1 line-clamp-2 text-sm">
                                        {skill.instructions}
                                    </p>
                                    <p class="text-muted-foreground mt-1 text-xs">
                                        Updated {formatDateTime(
                                            skill.updatedAt,
                                            data.user.configuration,
                                        )}
                                    </p>
                                </div>
                                <div
                                    class="invisible ml-4 flex shrink-0 items-center gap-1 group-hover:visible">
                                    <Tooltip>
                                        <TooltipTrigger>
                                            <Button
                                                variant="ghost"
                                                size="icon"
                                                class="cursor-pointer"
                                                onclick={() => openEdit(skill)}>
                                                <Pencil class="h-4 w-4" />
                                            </Button>
                                        </TooltipTrigger>
                                        <TooltipContent>Edit</TooltipContent>
                                    </Tooltip>
                                    <Tooltip>
                                        <TooltipTrigger>
                                            <Button
                                                variant="ghost"
                                                size="icon"
                                                class="cursor-pointer text-red-500 hover:text-red-600"
                                                onclick={() => openDelete(skill)}>
                                                <Trash2 class="h-4 w-4" />
                                            </Button>
                                        </TooltipTrigger>
                                        <TooltipContent>Delete</TooltipContent>
                                    </Tooltip>
                                </div>
                            </Card.Content>
                        </Card.Root>
                    {/each}
                </div>
            {/if}
        </Tabs.Content>

        <!-- Public Library tab -->
        <Tabs.Content value="public">
            {#if publicSkills.length === 0}
                <div
                    class="flex flex-col items-center justify-center rounded-lg border border-dashed p-12 text-center">
                    <BookOpen class="text-muted-foreground mb-4 h-12 w-12" />
                    <h3 class="mb-2 text-lg font-medium">No public skills yet</h3>
                    <p class="text-muted-foreground text-sm">
                        Public skills created by other users will appear here. You can clone any
                        public skill to make it your own.
                    </p>
                </div>
            {:else}
                <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
                    {#each publicSkills as skill (skill.id)}
                        <Card.Root class="group hover:bg-muted/50 transition-colors">
                            <Card.Content class="flex items-start justify-between">
                                <div class="min-w-0 flex-1">
                                    <div class="flex items-center gap-2">
                                        <h3 class="font-medium">{skill.name}</h3>
                                        <Badge variant="outline">
                                            <Globe class="mr-1 h-3 w-3" />
                                            Public
                                        </Badge>
                                        <Badge variant="secondary">Shared</Badge>
                                    </div>
                                    <p class="text-muted-foreground mt-1 line-clamp-2 text-sm">
                                        {skill.instructions}
                                    </p>
                                    <p class="text-muted-foreground mt-1 text-xs">
                                        ID: library:{skill.id}
                                    </p>
                                </div>
                                <div class="invisible ml-4 shrink-0 group-hover:visible">
                                    <Button
                                        variant="outline"
                                        size="sm"
                                        class="cursor-pointer"
                                        disabled={cloningIds.includes(skill.id)}
                                        onclick={() => handleClone(skill.id)}>
                                        <Copy class="mr-1 h-3 w-3" />
                                        {cloningIds.includes(skill.id) ? 'Cloning...' : 'Clone'}
                                    </Button>
                                </div>
                            </Card.Content>
                        </Card.Root>
                    {/each}
                </div>
            {/if}
        </Tabs.Content>
    </Tabs.Root>
</div>

<!-- Delete confirmation dialog -->
<AlertDialog.Root
    open={showDeleteConfirm}
    onOpenChange={(open) => {
        if (!open) {
            showDeleteConfirm = false
            deletingSkill = null
        }
    }}>
    <AlertDialog.Content>
        <AlertDialog.Header>
            <AlertDialog.Title>Delete skill</AlertDialog.Title>
            <AlertDialog.Description>
                This will permanently delete "{deletingSkill?.name}". This action cannot be undone.
            </AlertDialog.Description>
        </AlertDialog.Header>
        <AlertDialog.Footer>
            <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
            <AlertDialog.Action onclick={confirmDelete}>Delete</AlertDialog.Action>
        </AlertDialog.Footer>
    </AlertDialog.Content>
</AlertDialog.Root>
