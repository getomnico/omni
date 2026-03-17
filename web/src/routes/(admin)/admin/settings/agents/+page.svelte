<script lang="ts">
    import { Button } from '$lib/components/ui/button/index.js'
    import { Badge } from '$lib/components/ui/badge/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Textarea } from '$lib/components/ui/textarea/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import * as Select from '$lib/components/ui/select/index.js'
    import * as Switch from '$lib/components/ui/switch/index.js'
    import * as Dialog from '$lib/components/ui/dialog/index.js'
    import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js'
    import { Plus, Play, Bot, Trash2 } from '@lucide/svelte'
    import { invalidateAll } from '$app/navigation'
    import { formatSchedule } from '$lib/utils/schedule.js'
    import type { PageData } from './$types.js'

    let { data }: { data: PageData } = $props()

    // Dialog state — shared for create and edit
    let showDialog = $state(false)
    let editingAgentId = $state<string | null>(null)
    let name = $state('')
    let instructions = $state('')
    let schedulePreset = $state('daily')
    let customCron = $state('')
    let allowedActions = $state('')
    let isEnabled = $state(true)
    let submitting = $state(false)
    let error = $state('')

    let showDeleteConfirm = $state(false)
    let deleteTargetId = $state<string | null>(null)

    let isEditing = $derived(editingAgentId !== null)

    const schedulePresets: Record<string, { type: string; value: string; label: string }> = {
        hourly: { type: 'interval', value: '3600', label: 'Every hour' },
        '6hours': { type: 'interval', value: '21600', label: 'Every 6 hours' },
        daily: { type: 'cron', value: '0 9 * * *', label: 'Daily at 9am' },
        weekly: { type: 'cron', value: '0 9 * * 1', label: 'Weekly Monday 9am' },
        custom: { type: 'cron', value: '', label: 'Custom cron' },
    }

    function openCreate() {
        editingAgentId = null
        name = ''
        instructions = ''
        schedulePreset = 'daily'
        customCron = ''
        allowedActions = ''
        isEnabled = true
        error = ''
        showDialog = true
    }

    function openEdit(agent: any) {
        editingAgentId = agent.id
        name = agent.name
        instructions = agent.instructions
        isEnabled = agent.isEnabled
        error = ''

        // Try to match schedule to a preset
        const matchedPreset = Object.entries(schedulePresets).find(
            ([key, p]) =>
                key !== 'custom' &&
                p.type === agent.scheduleType &&
                p.value === agent.scheduleValue,
        )
        if (matchedPreset) {
            schedulePreset = matchedPreset[0]
            customCron = ''
        } else {
            schedulePreset = 'custom'
            customCron = agent.scheduleValue
        }

        const actions = agent.allowedActions as string[]
        allowedActions = Array.isArray(actions) ? actions.join(', ') : ''

        showDialog = true
    }

    async function handleSubmit() {
        if (!name.trim() || !instructions.trim()) {
            error = 'Name and instructions are required'
            return
        }

        const preset = schedulePresets[schedulePreset]
        const scheduleType = preset.type
        const scheduleValue = schedulePreset === 'custom' ? customCron : preset.value

        if (!scheduleValue) {
            error = 'Schedule is required'
            return
        }

        submitting = true
        error = ''

        const parsedActions = allowedActions
            .split(',')
            .map((a) => a.trim())
            .filter(Boolean)

        try {
            if (isEditing) {
                const res = await fetch(`/api/agents/${editingAgentId}`, {
                    method: 'PUT',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        name: name.trim(),
                        instructions: instructions.trim(),
                        scheduleType,
                        scheduleValue,
                        allowedActions: parsedActions,
                        isEnabled,
                    }),
                })
                if (!res.ok) {
                    const result = await res.json()
                    error = result.error || 'Failed to update agent'
                    return
                }
            } else {
                const res = await fetch('/api/agents', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        name: name.trim(),
                        instructions: instructions.trim(),
                        agentType: 'org',
                        scheduleType,
                        scheduleValue,
                        allowedActions: parsedActions,
                    }),
                })
                if (!res.ok) {
                    const result = await res.json()
                    error = result.error || 'Failed to create agent'
                    return
                }
            }

            showDialog = false
            invalidateAll()
        } catch {
            error = isEditing ? 'Failed to update agent' : 'Failed to create agent'
        } finally {
            submitting = false
        }
    }

    async function confirmDelete() {
        if (!deleteTargetId) return
        await fetch(`/api/agents/${deleteTargetId}`, { method: 'DELETE' })
        deleteTargetId = null
        showDeleteConfirm = false
        showDialog = false
        invalidateAll()
    }

    async function triggerAgent(agentId: string) {
        await fetch(`/api/agents/${agentId}/trigger`, { method: 'POST' })
    }
</script>

<!-- Delete confirmation -->
<AlertDialog.Root
    open={showDeleteConfirm}
    onOpenChange={(open) => {
        if (!open) {
            showDeleteConfirm = false
            deleteTargetId = null
        }
    }}>
    <AlertDialog.Content>
        <AlertDialog.Header>
            <AlertDialog.Title>Delete org agent</AlertDialog.Title>
            <AlertDialog.Description>
                This will permanently delete this agent. This action cannot be undone.
            </AlertDialog.Description>
        </AlertDialog.Header>
        <AlertDialog.Footer>
            <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
            <AlertDialog.Action onclick={confirmDelete}>Delete</AlertDialog.Action>
        </AlertDialog.Footer>
    </AlertDialog.Content>
</AlertDialog.Root>

<!-- Create / Edit dialog -->
<Dialog.Root
    open={showDialog}
    onOpenChange={(open) => {
        showDialog = open
    }}>
    <Dialog.Content class="max-w-lg">
        <Dialog.Header>
            <Dialog.Title>{isEditing ? 'Edit Org Agent' : 'Create Org Agent'}</Dialog.Title>
            <Dialog.Description>
                Org agents have read access to all data sources. Write actions are restricted to an
                explicit whitelist.
            </Dialog.Description>
        </Dialog.Header>
        <form
            onsubmit={(e) => {
                e.preventDefault()
                handleSubmit()
            }}
            class="space-y-4">
            <div class="space-y-2">
                <Label for="org-name">Name</Label>
                <Input id="org-name" bind:value={name} placeholder="e.g., Weekly Company Summary" />
            </div>
            <div class="space-y-2">
                <Label for="org-instructions">Instructions</Label>
                <Textarea
                    id="org-instructions"
                    bind:value={instructions}
                    rows={4}
                    placeholder="Describe the task..." />
            </div>
            <div class="space-y-2">
                <Label>Schedule</Label>
                <Select.Root
                    type="single"
                    value={schedulePreset}
                    onValueChange={(v) => {
                        schedulePreset = v
                    }}>
                    <Select.Trigger class="cursor-pointer">
                        {schedulePresets[schedulePreset]?.label || 'Select'}
                    </Select.Trigger>
                    <Select.Content>
                        {#each Object.entries(schedulePresets) as [key, preset]}
                            <Select.Item value={key} class="cursor-pointer"
                                >{preset.label}</Select.Item>
                        {/each}
                    </Select.Content>
                </Select.Root>
                {#if schedulePreset === 'custom'}
                    <Input bind:value={customCron} placeholder="*/30 * * * *" class="mt-2" />
                {/if}
            </div>
            <div class="space-y-2">
                <Label for="org-actions">Allowed Write Actions</Label>
                <Input
                    id="org-actions"
                    bind:value={allowedActions}
                    placeholder="gmail__send_email, slack__post_message" />
                <p class="text-muted-foreground text-xs">
                    Comma-separated list of tool names this agent can use for write operations. Read
                    access to all sources is granted by default.
                </p>
            </div>

            {#if isEditing}
                <div class="flex items-center justify-between">
                    <Label>Enabled</Label>
                    <Switch.Root bind:checked={isEnabled} class="cursor-pointer" />
                </div>
            {/if}

            <div
                class="space-y-2 rounded-lg border border-amber-200 bg-amber-50 p-3 text-xs text-amber-800 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-300">
                <p class="font-semibold">
                    Exercise caution when creating org agents. They have read access to all data
                    across the organization, including documents belonging to all users.
                </p>
                <p>
                    Execution logs are not visible to anyone, including admins. Only status, timing,
                    and summary are shown.
                </p>
            </div>

            {#if error}
                <p class="text-sm text-red-500">{error}</p>
            {/if}

            <Dialog.Footer>
                {#if isEditing}
                    <Button
                        variant="destructive"
                        size="sm"
                        class="mr-auto cursor-pointer"
                        onclick={() => {
                            deleteTargetId = editingAgentId
                            showDeleteConfirm = true
                        }}>
                        <Trash2 class="mr-1 h-3 w-3" /> Delete
                    </Button>
                {/if}
                <Button
                    variant="outline"
                    onclick={() => {
                        showDialog = false
                    }}>Cancel</Button>
                <Button type="submit" disabled={submitting} class="cursor-pointer">
                    {submitting
                        ? isEditing
                            ? 'Saving...'
                            : 'Creating...'
                        : isEditing
                          ? 'Save'
                          : 'Create'}
                </Button>
            </Dialog.Footer>
        </form>
    </Dialog.Content>
</Dialog.Root>

<div class="mx-auto max-w-4xl p-6">
    <div class="mb-6 flex items-center justify-between">
        <div>
            <h1 class="text-2xl font-bold">Org Agents</h1>
            <p class="text-muted-foreground text-sm">
                Organization-level agents with read access to all data sources
            </p>
        </div>
        <Button class="cursor-pointer" onclick={openCreate}>
            <Plus class="mr-2 h-4 w-4" />
            New Org Agent
        </Button>
    </div>

    {#if data.agents.length === 0}
        <div
            class="flex flex-col items-center justify-center rounded-lg border border-dashed p-12 text-center">
            <Bot class="text-muted-foreground mb-4 h-12 w-12" />
            <h3 class="mb-2 text-lg font-medium">No org agents</h3>
            <p class="text-muted-foreground mb-4 text-sm">
                Create an org agent for cross-team automation tasks.
            </p>
        </div>
    {:else}
        <div class="space-y-3">
            {#each data.agents as agent (agent.id)}
                <div
                    class="hover:bg-muted/50 flex items-center justify-between rounded-lg border p-4 transition-colors">
                    <button
                        class="min-w-0 flex-1 cursor-pointer text-left"
                        onclick={() => openEdit(agent)}>
                        <div class="flex items-center gap-2">
                            <h3 class="font-medium">{agent.name}</h3>
                            <Badge variant={agent.isEnabled ? 'default' : 'secondary'}>
                                {agent.isEnabled ? 'Active' : 'Paused'}
                            </Badge>
                        </div>
                        <p class="text-muted-foreground mt-1 truncate text-sm">
                            {agent.instructions}
                        </p>
                        <p class="text-muted-foreground mt-1 text-xs">
                            Schedule: {formatSchedule(agent.scheduleType, agent.scheduleValue)}
                        </p>
                    </button>
                    <Button
                        variant="ghost"
                        size="icon"
                        class="ml-4 cursor-pointer"
                        onclick={() => triggerAgent(agent.id)}>
                        <Play class="h-4 w-4" />
                    </Button>
                </div>
            {/each}
        </div>
    {/if}
</div>
