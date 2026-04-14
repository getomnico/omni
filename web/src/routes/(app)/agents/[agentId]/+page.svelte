<script lang="ts">
    import { Button } from '$lib/components/ui/button/index.js'
    import { Badge } from '$lib/components/ui/badge/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Textarea } from '$lib/components/ui/textarea/index.js'
    import { Label } from '$lib/components/ui/label/index.js'
    import * as Switch from '$lib/components/ui/switch/index.js'
    import { Play, Trash2, Save, MessageSquare } from '@lucide/svelte'
    import { goto, invalidateAll } from '$app/navigation'
    import { formatSchedule } from '$lib/utils/schedule.js'
    import * as Select from '$lib/components/ui/select/index.js'
    import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js'
    import { formatProviderName } from '$lib/utils/providers.js'
    import type { PageData } from './$types.js'

    let { data }: { data: PageData } = $props()

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

    let editName = $state(data.agent.name)
    let editInstructions = $state(data.agent.instructions)
    let editModelId = $state<string | undefined>(data.agent.modelId ?? undefined)
    let showDelete = $state(false)
    let saving = $state(false)
    let startingChat = $state(false)

    async function startChat() {
        startingChat = true
        try {
            const res = await fetch(`/api/agents/${data.agent.id}/chat`, { method: 'POST' })
            if (res.ok) {
                const { chatId } = await res.json()
                goto(`/chat/${chatId}`, { state: { stream: true } })
            }
        } finally {
            startingChat = false
        }
    }

    async function save() {
        saving = true
        await fetch(`/api/agents/${data.agent.id}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                name: editName.trim(),
                instructions: editInstructions.trim(),
                modelId: editModelId || null,
            }),
        })
        saving = false
        invalidateAll()
    }

    async function toggleEnabled() {
        await fetch(`/api/agents/${data.agent.id}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ isEnabled: !data.agent.isEnabled }),
        })
        invalidateAll()
    }

    async function triggerRun() {
        await fetch(`/api/agents/${data.agent.id}/trigger`, { method: 'POST' })
        invalidateAll()
    }

    async function confirmDelete() {
        await fetch(`/api/agents/${data.agent.id}`, { method: 'DELETE' })
        goto('/agents')
    }

    function formatDate(date: Date | string | null): string {
        if (!date) return '—'
        return new Date(date).toLocaleString()
    }

    function statusColor(status: string): 'default' | 'secondary' | 'destructive' | 'outline' {
        switch (status) {
            case 'completed':
                return 'default'
            case 'running':
                return 'secondary'
            case 'failed':
                return 'destructive'
            default:
                return 'outline'
        }
    }
</script>

<AlertDialog.Root
    open={showDelete}
    onOpenChange={(open) => {
        showDelete = open
    }}>
    <AlertDialog.Content>
        <AlertDialog.Header>
            <AlertDialog.Title>Delete agent</AlertDialog.Title>
            <AlertDialog.Description>
                This will permanently delete "{data.agent.name}". Existing run history will be
                preserved.
            </AlertDialog.Description>
        </AlertDialog.Header>
        <AlertDialog.Footer>
            <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
            <AlertDialog.Action onclick={confirmDelete}>Delete</AlertDialog.Action>
        </AlertDialog.Footer>
    </AlertDialog.Content>
</AlertDialog.Root>

<div class="mx-auto max-w-4xl p-6">
    <!-- Header -->
    <div class="mb-6 flex items-center justify-between">
        <div class="flex items-center gap-3">
            <h1 class="text-2xl font-bold">{data.agent.name}</h1>
            <Badge variant={data.agent.isEnabled ? 'default' : 'secondary'}>
                {data.agent.isEnabled ? 'Active' : 'Paused'}
            </Badge>
        </div>
        <div class="flex items-center gap-2">
            <Button
                variant="outline"
                size="sm"
                class="cursor-pointer"
                onclick={startChat}
                disabled={startingChat}>
                <MessageSquare class="mr-1 h-3 w-3" />
                {startingChat ? 'Starting...' : 'Chat with Agent'}
            </Button>
            <Button variant="outline" size="sm" class="cursor-pointer" onclick={triggerRun}>
                <Play class="mr-1 h-3 w-3" /> Run Now
            </Button>
            <Switch.Root
                checked={data.agent.isEnabled}
                onCheckedChange={toggleEnabled}
                class="cursor-pointer" />
            <Button
                variant="ghost"
                size="icon"
                class="text-destructive cursor-pointer"
                onclick={() => {
                    showDelete = true
                }}>
                <Trash2 class="h-4 w-4" />
            </Button>
        </div>
    </div>

    <!-- Config editing -->
    <div class="mb-8 space-y-4 rounded-lg border p-4">
        <div class="space-y-2">
            <Label for="name">Name</Label>
            <Input id="name" bind:value={editName} />
        </div>
        <div class="space-y-2">
            <Label for="instructions">Instructions</Label>
            <Textarea id="instructions" bind:value={editInstructions} rows={4} />
        </div>
        {#if data.models.length > 0}
            <div class="space-y-2">
                <Label>Model</Label>
                <Select.Root
                    type="single"
                    value={editModelId}
                    onValueChange={(v) => {
                        editModelId = v
                    }}>
                    <Select.Trigger class="cursor-pointer">
                        {data.models.find((m) => m.id === editModelId)?.displayName || 'Default'}
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
            </div>
        {/if}
        <div class="text-muted-foreground text-sm">
            Schedule: {formatSchedule(data.agent.scheduleType, data.agent.scheduleValue)}
        </div>
        <Button size="sm" class="cursor-pointer" onclick={save} disabled={saving}>
            <Save class="mr-1 h-3 w-3" />
            {saving ? 'Saving...' : 'Save Changes'}
        </Button>
    </div>

    <!-- Run history -->
    <h2 class="mb-4 text-lg font-semibold">Run History</h2>
    {#if data.runs.length === 0}
        <p class="text-muted-foreground text-sm">
            No runs yet. Click "Run Now" to trigger the first run.
        </p>
    {:else}
        <div class="overflow-hidden rounded-lg border">
            <table class="w-full text-sm">
                <thead>
                    <tr class="bg-muted/50 border-b">
                        <th class="px-4 py-2 text-left font-medium">Status</th>
                        <th class="px-4 py-2 text-left font-medium">Started</th>
                        <th class="px-4 py-2 text-left font-medium">Completed</th>
                        <th class="px-4 py-2 text-left font-medium">Summary</th>
                    </tr>
                </thead>
                <tbody>
                    {#each data.runs as run (run.id)}
                        <tr class="hover:bg-muted/30 border-b last:border-0">
                            <td class="px-4 py-2">
                                <a
                                    href="/agents/{data.agent.id}/runs/{run.id}"
                                    class="cursor-pointer">
                                    <Badge variant={statusColor(run.status)}>{run.status}</Badge>
                                </a>
                            </td>
                            <td class="px-4 py-2">{formatDate(run.startedAt)}</td>
                            <td class="px-4 py-2">{formatDate(run.completedAt)}</td>
                            <td class="max-w-md truncate px-4 py-2">
                                <a
                                    href="/agents/{data.agent.id}/runs/{run.id}"
                                    class="cursor-pointer hover:underline">
                                    {run.summary || run.errorMessage || '—'}
                                </a>
                            </td>
                        </tr>
                    {/each}
                </tbody>
            </table>
        </div>
    {/if}
</div>
