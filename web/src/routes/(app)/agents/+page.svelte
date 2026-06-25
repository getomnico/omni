<script lang="ts">
    import { Button } from '$lib/components/ui/button/index.js'
    import { Badge } from '$lib/components/ui/badge/index.js'
    import * as Switch from '$lib/components/ui/switch/index.js'
    import { Plus, Play, Bot } from '@lucide/svelte'
    import { goto, invalidateAll } from '$app/navigation'
    import { formatSchedule } from '$lib/utils/schedule.js'
    import type { PageData } from './$types.js'

    let { data }: { data: PageData } = $props()

    async function toggleAgent(agentId: string, enabled: boolean) {
        await fetch(`/api/agents/${agentId}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ isEnabled: !enabled }),
        })
        invalidateAll()
    }

    async function triggerAgent(agentId: string) {
        await fetch(`/api/agents/${agentId}/trigger`, { method: 'POST' })
        invalidateAll()
    }

    function activeLabel(agentId: string): string | null {
        const run = data.activeRuns?.[agentId]
        if (!run) return null
        return run.status === 'pending' ? 'Queued' : 'Running'
    }
</script>

<div class="mx-auto max-w-4xl p-6">
    <div class="mb-6 flex items-center justify-between">
        <div>
            <h1 class="text-2xl font-bold">Agents</h1>
            <p class="text-muted-foreground text-sm">
                Automated background tasks that run on a schedule
            </p>
        </div>
        <Button href="/agents/new" class="cursor-pointer">
            <Plus class="mr-2 h-4 w-4" />
            New Agent
        </Button>
    </div>

    {#if data.agents.length === 0}
        <div
            class="flex flex-col items-center justify-center rounded-lg border border-dashed p-12 text-center">
            <Bot class="text-muted-foreground mb-4 h-12 w-12" />
            <h3 class="mb-2 text-lg font-medium">No agents yet</h3>
            <p class="text-muted-foreground mb-4 text-sm">
                Create an agent to automate tasks like daily summaries, monitoring, or report
                generation.
            </p>
            <Button href="/agents/new" class="cursor-pointer">
                <Plus class="mr-2 h-4 w-4" />
                Create your first agent
            </Button>
        </div>
    {:else}
        <div class="space-y-3">
            {#each data.agents as agent (agent.id)}
                {@const label = activeLabel(agent.id)}
                <div
                    class="hover:bg-muted/50 flex items-center justify-between rounded-lg border p-4 transition-colors">
                    <a href="/agents/{agent.id}" class="min-w-0 flex-1 cursor-pointer">
                        <div class="flex items-center gap-3">
                            <div class="min-w-0 flex-1">
                                <div class="flex items-center gap-2">
                                    <h3 class="font-medium">{agent.name}</h3>
                                    {#if agent.agentType === 'org'}
                                        <Badge variant="outline">Org</Badge>
                                    {/if}
                                    <Badge variant={agent.isEnabled ? 'default' : 'secondary'}>
                                        {agent.isEnabled ? 'Active' : 'Paused'}
                                    </Badge>
                                </div>
                                <p class="text-muted-foreground mt-1 truncate text-sm">
                                    {agent.instructions}
                                </p>
                                <p class="text-muted-foreground mt-1 text-xs">
                                    Schedule: {formatSchedule(
                                        agent.scheduleType,
                                        agent.scheduleValue,
                                    )}
                                </p>
                            </div>
                        </div>
                    </a>
                    <div class="ml-4 flex items-center gap-2">
                        <Button
                            variant="ghost"
                            size={label ? 'sm' : 'icon'}
                            class="cursor-pointer"
                            disabled={!!label}
                            onclick={() => triggerAgent(agent.id)}>
                            {#if label}
                                {label}
                            {:else}
                                <Play class="h-4 w-4" />
                            {/if}
                        </Button>
                        <Switch.Root
                            checked={agent.isEnabled}
                            onCheckedChange={() => toggleAgent(agent.id, agent.isEnabled)}
                            class="cursor-pointer" />
                    </div>
                </div>
            {/each}
        </div>
    {/if}
</div>
