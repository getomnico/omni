<script lang="ts">
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Textarea } from '$lib/components/ui/textarea/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import * as Select from '$lib/components/ui/select/index.js'
    import { Checkbox } from '$lib/components/ui/checkbox/index.js'
    import { goto } from '$app/navigation'
    import { formatProviderName } from '$lib/utils/providers.js'
    import type { PageData } from './$types.js'

    let { data }: { data: PageData } = $props()

    let name = $state('')
    let instructions = $state('')
    let schedulePreset = $state('hourly')
    let customCron = $state('')
    let selectedSources = $state<Record<string, { read: boolean; write: boolean }>>({})
    // Falls back to the workspace default model so runs don't fail with
    // "Agent has no model configured" when the field is left untouched.
    let selectedModelOverride = $state<string | undefined>(undefined)
    let selectedModelId = $derived(
        selectedModelOverride ?? data.models.find((m) => m.isDefault)?.id,
    )
    let submitting = $state(false)
    let error = $state('')

    let groupedModels = $derived(
        Object.entries(
            data.models.reduce<Record<string, typeof data.models>>((acc, m) => {
                const key = m.providerType
                if (!acc[key]) acc[key] = []
                acc[key].push(m)
                return acc
            }, {}),
        ),
    )

    const schedulePresets: Record<string, { type: string; value: string; label: string }> = {
        hourly: { type: 'interval', value: '3600', label: 'Every hour' },
        '6hours': { type: 'interval', value: '21600', label: 'Every 6 hours' },
        daily: { type: 'cron', value: '0 9 * * *', label: 'Daily at 9am' },
        weekly: { type: 'cron', value: '0 9 * * 1', label: 'Weekly Monday 9am' },
        custom: { type: 'cron', value: '', label: 'Custom cron' },
    }

    function toggleSource(sourceId: string) {
        if (selectedSources[sourceId]) {
            const { [sourceId]: _, ...rest } = selectedSources
            selectedSources = rest
        } else {
            selectedSources = { ...selectedSources, [sourceId]: { read: true, write: false } }
        }
    }

    function toggleSourceMode(sourceId: string, mode: 'read' | 'write') {
        if (!selectedSources[sourceId]) return
        selectedSources = {
            ...selectedSources,
            [sourceId]: {
                ...selectedSources[sourceId],
                [mode]: !selectedSources[sourceId][mode],
            },
        }
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

        const allowedSources = Object.entries(selectedSources).map(([sourceId, modes]) => ({
            source_id: sourceId,
            modes: [...(modes.read ? ['read'] : []), ...(modes.write ? ['write'] : [])],
        }))

        submitting = true
        error = ''

        try {
            const res = await fetch('/api/agents', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: name.trim(),
                    instructions: instructions.trim(),
                    agentType: 'user',
                    scheduleType,
                    scheduleValue,
                    allowedSources,
                    modelId: selectedModelId || undefined,
                }),
            })

            if (res.ok) {
                const agent = await res.json()
                goto(`/agents/${agent.id}`)
            } else {
                const result = await res.json()
                error = result.error || 'Failed to create agent'
            }
        } catch {
            error = 'Failed to create agent'
        } finally {
            submitting = false
        }
    }
</script>

<div class="mx-auto max-w-2xl p-6">
    <h1 class="mb-6 text-2xl font-bold">Create Agent</h1>

    <form
        onsubmit={(e) => {
            e.preventDefault()
            handleSubmit()
        }}
        class="space-y-6">
        <div class="space-y-2">
            <Label for="name">Name</Label>
            <Input id="name" bind:value={name} placeholder="e.g., Daily Standup Summary" />
        </div>

        <div class="space-y-2">
            <Label for="instructions">Instructions</Label>
            <Textarea
                id="instructions"
                bind:value={instructions}
                class="max-h-96 overflow-y-auto"
                placeholder="Describe what this agent should do. For example: Search Slack for messages from the #engineering channel from the past 24 hours and summarize the key discussion points."
                rows={6} />
            <p class="text-muted-foreground text-xs">
                This becomes the agent's task description. Be specific about what to search,
                analyze, or do.
            </p>
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
                    {schedulePresets[schedulePreset]?.label || 'Select schedule'}
                </Select.Trigger>
                <Select.Content>
                    {#each Object.entries(schedulePresets) as [key, preset]}
                        <Select.Item value={key} class="cursor-pointer">{preset.label}</Select.Item>
                    {/each}
                </Select.Content>
            </Select.Root>
            {#if schedulePreset === 'custom'}
                <Input bind:value={customCron} placeholder="*/30 * * * *" class="mt-2" />
                <p class="text-muted-foreground text-xs">Enter a cron expression</p>
            {/if}
        </div>

        {#if data.models.length > 0}
            <div class="space-y-2">
                <Label>Model</Label>
                <Select.Root
                    type="single"
                    value={selectedModelId}
                    onValueChange={(v) => {
                        selectedModelId = v
                    }}>
                    <Select.Trigger class="cursor-pointer">
                        {data.models.find((m) => m.id === selectedModelId)?.displayName ||
                            'Default'}
                    </Select.Trigger>
                    <Select.Content>
                        {#each groupedModels as [provider, providerModels]}
                            <Select.Group>
                                <Select.GroupHeading>
                                    {formatProviderName(provider)}
                                </Select.GroupHeading>
                                {#each providerModels as model}
                                    <Select.Item value={model.id} class="cursor-pointer">
                                        {model.displayName}
                                    </Select.Item>
                                {/each}
                            </Select.Group>
                        {/each}
                    </Select.Content>
                </Select.Root>
                <p class="text-muted-foreground text-xs">
                    Preselected to your workspace's default model.
                </p>
            </div>
        {/if}

        {#if data.sources.length > 0}
            <div class="space-y-2">
                <Label>App Access</Label>
                <p class="text-muted-foreground text-xs">
                    Select which connected apps this agent can access
                </p>
                <div class="space-y-2 rounded-lg border p-3">
                    {#each data.sources as source}
                        <div class="flex items-center justify-between">
                            <div class="flex items-center gap-2">
                                <Checkbox
                                    checked={!!selectedSources[source.id]}
                                    onCheckedChange={() => toggleSource(source.id)} />
                                <span class="text-sm">{source.name}</span>
                                <span class="text-muted-foreground text-xs"
                                    >({source.source_type})</span>
                            </div>
                            {#if selectedSources[source.id]}
                                <div class="flex items-center gap-3 text-xs">
                                    <label class="flex cursor-pointer items-center gap-1">
                                        <Checkbox
                                            checked={selectedSources[source.id]?.read}
                                            onCheckedChange={() =>
                                                toggleSourceMode(source.id, 'read')} />
                                        Read
                                    </label>
                                    <label class="flex cursor-pointer items-center gap-1">
                                        <Checkbox
                                            checked={selectedSources[source.id]?.write}
                                            onCheckedChange={() =>
                                                toggleSourceMode(source.id, 'write')} />
                                        Write
                                    </label>
                                </div>
                            {/if}
                        </div>
                    {/each}
                </div>
            </div>
        {/if}

        {#if error}
            <p class="text-sm text-red-500">{error}</p>
        {/if}

        <div class="flex gap-3">
            <Button type="submit" disabled={submitting} class="cursor-pointer">
                {submitting ? 'Creating...' : 'Create Agent'}
            </Button>
            <Button variant="outline" href="/agents" class="cursor-pointer">Cancel</Button>
        </div>
    </form>
</div>
