<script lang="ts">
    import { Button } from '$lib/components/ui/button/index.js'
    import { Badge } from '$lib/components/ui/badge/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Textarea } from '$lib/components/ui/textarea/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import * as Tabs from '$lib/components/ui/tabs/index.js'
    import * as Card from '$lib/components/ui/card/index.js'
    import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js'
    import { Plus, BookOpen, Copy, Pencil, Trash2, Globe, Lock } from '@lucide/svelte'
    import { invalidateAll } from '$app/navigation'
    import { toast } from 'svelte-sonner'
    import { formatDateTime } from '$lib/utils/datetime'
    import type { PageData } from './$types.js'
    import type { Skill } from '$lib/server/db/schema.js'
    import type { SkillVisibility } from '$lib/skills.js'

    let { data }: { data: PageData } = $props()

    let tab = $state('mine')

    let showNewForm = $state(false)
    let newName = $state('')
    let newInstructions = $state('')
    let newVisibility = $state<SkillVisibility>('private')
    let saving = $state(false)
    let filterQuery = $state('')

    let showEditForm = $state(false)
    let editingSkill = $state<Skill | null>(null)
    let editName = $state('')
    let editInstructions = $state('')
    let editVisibility = $state<SkillVisibility>('private')

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
        showNewForm = false
        newName = ''
        newInstructions = ''
        newVisibility = 'private'
    }

    function openEdit(skill: Skill) {
        editingSkill = skill
        editName = skill.name
        editInstructions = skill.instructions
        editVisibility = skill.visibility
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
                    visibility: newVisibility,
                }),
            })
            if (res.ok) {
                resetNewForm()
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
                    visibility: editVisibility,
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
                Create and manage workplace skills. Public skills are visible to everyone in your
                deployment.
            </p>
        </div>
        {#if !showNewForm}
            <Button
                class="cursor-pointer"
                onclick={() => {
                    resetNewForm()
                    showNewForm = true
                }}>
                <Plus class="mr-2 h-4 w-4" />
                New Skill
            </Button>
        {/if}
    </div>

    <!-- Create form -->
    {#if showNewForm}
        <Card.Root class="mb-6">
            <Card.Header>
                <Card.Title>Create Skill</Card.Title>
                <Card.Description>
                    Write instructions that the AI will use when this skill is loaded.
                </Card.Description>
            </Card.Header>
            <Card.Content>
                <form
                    onsubmit={(e) => {
                        e.preventDefault()
                        handleCreate()
                    }}
                    class="space-y-4">
                    <div class="space-y-2">
                        <Label for="new-name">Name</Label>
                        <Input
                            id="new-name"
                            bind:value={newName}
                            placeholder="e.g., PR Review Checklist" />
                    </div>
                    <div class="space-y-2">
                        <Label for="new-instructions">Instructions</Label>
                        <Textarea
                            id="new-instructions"
                            bind:value={newInstructions}
                            placeholder="Describe the task, what tools to use, and how to format the response..."
                            rows={6} />
                    </div>
                    <div class="space-y-2">
                        <Label>Visibility</Label>
                        <div class="flex gap-4">
                            <label class="flex cursor-pointer items-center gap-2">
                                <input
                                    type="radio"
                                    name="visibility"
                                    value="private"
                                    checked={newVisibility === 'private'}
                                    onchange={() => (newVisibility = 'private')} />
                                <Lock class="h-4 w-4" />
                                <span class="text-sm">Private (only you)</span>
                            </label>
                            <label class="flex cursor-pointer items-center gap-2">
                                <input
                                    type="radio"
                                    name="visibility"
                                    value="public"
                                    checked={newVisibility === 'public'}
                                    onchange={() => (newVisibility = 'public')} />
                                <Globe class="h-4 w-4" />
                                <span class="text-sm">Public (everyone)</span>
                            </label>
                        </div>
                    </div>
                    <div class="flex gap-2">
                        <Button type="submit" disabled={saving} class="cursor-pointer">
                            {saving ? 'Creating...' : 'Create'}
                        </Button>
                        <Button
                            variant="outline"
                            class="cursor-pointer"
                            onclick={resetNewForm}
                            type="button">
                            Cancel
                        </Button>
                    </div>
                </form>
            </Card.Content>
        </Card.Root>
    {/if}

    <!-- Edit form dialog -->
    {#if showEditForm && editingSkill}
        <Card.Root class="mb-6">
            <Card.Header>
                <Card.Title>Edit Skill</Card.Title>
            </Card.Header>
            <Card.Content>
                <form
                    onsubmit={(e) => {
                        e.preventDefault()
                        handleUpdate()
                    }}
                    class="space-y-4">
                    <div class="space-y-2">
                        <Label for="edit-name">Name</Label>
                        <Input id="edit-name" bind:value={editName} />
                    </div>
                    <div class="space-y-2">
                        <Label for="edit-instructions">Instructions</Label>
                        <Textarea id="edit-instructions" bind:value={editInstructions} rows={6} />
                    </div>
                    <div class="space-y-2">
                        <Label>Visibility</Label>
                        <div class="flex gap-4">
                            <label class="flex cursor-pointer items-center gap-2">
                                <input
                                    type="radio"
                                    name="edit-visibility"
                                    value="private"
                                    checked={editVisibility === 'private'}
                                    onchange={() => (editVisibility = 'private')} />
                                <Lock class="h-4 w-4" />
                                <span class="text-sm">Private</span>
                            </label>
                            <label class="flex cursor-pointer items-center gap-2">
                                <input
                                    type="radio"
                                    name="edit-visibility"
                                    value="public"
                                    checked={editVisibility === 'public'}
                                    onchange={() => (editVisibility = 'public')} />
                                <Globe class="h-4 w-4" />
                                <span class="text-sm">Public</span>
                            </label>
                        </div>
                    </div>
                    <div class="flex gap-2">
                        <Button type="submit" disabled={saving} class="cursor-pointer">
                            {saving ? 'Saving...' : 'Save Changes'}
                        </Button>
                        <Button
                            variant="outline"
                            class="cursor-pointer"
                            onclick={() => {
                                showEditForm = false
                                editingSkill = null
                            }}
                            type="button">
                            Cancel
                        </Button>
                    </div>
                </form>
            </Card.Content>
        </Card.Root>
    {/if}

    <div class="mb-4">
        <Label for="skill-filter">Filter skills</Label>
        <Input
            id="skill-filter"
            bind:value={filterQuery}
            placeholder="Search by name, instructions, or library:<id>"
            class="mt-2" />
    </div>

    <!-- Tabs -->
    <Tabs.Root value={tab} onValueChange={(v) => (tab = v)} class="w-full">
        <Tabs.List class="mb-4">
            <Tabs.Trigger value="mine" class="cursor-pointer">
                My Skills ({mySkills.length})
            </Tabs.Trigger>
            <Tabs.Trigger value="public" class="cursor-pointer">
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
                <div class="space-y-3">
                    {#each mySkills as skill (skill.id)}
                        <Card.Root class="hover:bg-muted/50 transition-colors">
                            <Card.Content class="flex items-start justify-between p-4">
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
                                <div class="ml-4 flex shrink-0 items-center gap-1">
                                    <Button
                                        variant="ghost"
                                        size="icon"
                                        class="cursor-pointer"
                                        onclick={() => openEdit(skill)}
                                        title="Edit">
                                        <Pencil class="h-4 w-4" />
                                    </Button>
                                    <Button
                                        variant="ghost"
                                        size="icon"
                                        class="cursor-pointer text-red-500 hover:text-red-600"
                                        onclick={() => openDelete(skill)}
                                        title="Delete">
                                        <Trash2 class="h-4 w-4" />
                                    </Button>
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
                <div class="space-y-3">
                    {#each publicSkills as skill (skill.id)}
                        <Card.Root class="hover:bg-muted/50 transition-colors">
                            <Card.Content class="flex items-start justify-between p-4">
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
                                <div class="ml-4 shrink-0">
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
