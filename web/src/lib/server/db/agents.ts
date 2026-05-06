import { db } from './index.js'
import { agents, agentRuns } from './schema.js'
import { eq, and, desc } from 'drizzle-orm'
import { ulid } from 'ulid'
import { error } from '@sveltejs/kit'
import type { Agent } from './schema.js'

/**
 * Fetch an agent by ID and verify the user has access.
 * Throws SvelteKit error (404/403) on failure.
 */
export async function requireAgentAccess(
    agentId: string,
    user: { id: string; role: string },
): Promise<Agent> {
    const agent = await getAgent(agentId)
    if (!agent) {
        throw error(404, 'Agent not found')
    }
    if (agent.agentType === 'org') {
        if (user.role !== 'admin') {
            throw error(403, 'Admin access required')
        }
    } else if (agent.userId !== user.id) {
        throw error(403, 'Access denied')
    }
    return agent
}

async function seedAgentMemory(agent: Agent) {
    if (!agent.instructions) return
    try {
        const { getConfig } = await import('../config.js')
        const { services } = getConfig()
        const resp = await fetch(
            `${services.aiServiceUrl}/memories/agent/${encodeURIComponent(agent.id)}/seed`,
            {
                method: 'POST',
                headers: {
                    'content-type': 'application/json',
                    'x-user-id': 'system',
                    'x-user-role': 'admin',
                },
                body: JSON.stringify({
                    name: agent.name,
                    instructions: agent.instructions,
                    schedule_type: agent.scheduleType,
                    schedule_value: agent.scheduleValue,
                }),
            },
        )
        if (!resp.ok && resp.status !== 503) {
            console.warn(`Agent memory seed returned ${resp.status} for ${agent.id}`)
        }
    } catch (err) {
        console.warn(`Agent memory seed failed for ${agent.id}:`, err)
    }
}

export async function createAgent(data: {
    userId: string
    name: string
    instructions: string
    agentType: string
    scheduleType: string
    scheduleValue: string
    modelId?: string
    allowedSources?: any[]
    allowedActions?: string[]
}) {
    const id = ulid()
    const [agent] = await db
        .insert(agents)
        .values({
            id,
            userId: data.userId,
            name: data.name,
            instructions: data.instructions,
            agentType: data.agentType,
            scheduleType: data.scheduleType,
            scheduleValue: data.scheduleValue,
            modelId: data.modelId || null,
            allowedSources: data.allowedSources || [],
            allowedActions: data.allowedActions || [],
        })
        .returning()
    await seedAgentMemory(agent)
    return agent
}

export async function updateAgent(
    agentId: string,
    data: Partial<{
        name: string
        instructions: string
        scheduleType: string
        scheduleValue: string
        modelId: string | null
        allowedSources: any[]
        allowedActions: string[]
        isEnabled: boolean
    }>,
) {
    const [agent] = await db
        .update(agents)
        .set({ ...data, updatedAt: new Date() })
        .where(and(eq(agents.id, agentId), eq(agents.isDeleted, false)))
        .returning()
    if (
        agent &&
        (data.name !== undefined ||
            data.instructions !== undefined ||
            data.scheduleType !== undefined ||
            data.scheduleValue !== undefined)
    ) {
        await seedAgentMemory(agent)
    }
    return agent
}

export async function deleteAgent(agentId: string) {
    const existing = await getAgent(agentId)
    const [agent] = await db
        .update(agents)
        .set({ isDeleted: true, isEnabled: false, updatedAt: new Date() })
        .where(eq(agents.id, agentId))
        .returning()

    // Purge the agent memory namespace. Best-effort — never block delete.
    if (existing) {
        try {
            const { getConfig } = await import('../config.js')
            const { services } = getConfig()
            const resp = await fetch(
                `${services.aiServiceUrl}/memories/agent/${encodeURIComponent(agentId)}`,
                {
                    method: 'DELETE',
                    headers: { 'x-user-id': 'system', 'x-user-role': 'admin' },
                },
            )
            if (!resp.ok && resp.status !== 503) {
                console.warn(`Agent memory purge returned ${resp.status} for ${agentId}`)
            }
        } catch (err) {
            console.warn(`Agent memory purge failed for ${agentId}:`, err)
        }
    }

    return agent
}

export async function getAgent(agentId: string) {
    const [agent] = await db
        .select()
        .from(agents)
        .where(and(eq(agents.id, agentId), eq(agents.isDeleted, false)))
        .limit(1)
    return agent || null
}

export async function listAgents(userId: string) {
    return db
        .select()
        .from(agents)
        .where(and(eq(agents.userId, userId), eq(agents.isDeleted, false)))
        .orderBy(desc(agents.createdAt))
}

export async function listOrgAgents() {
    return db
        .select()
        .from(agents)
        .where(and(eq(agents.agentType, 'org'), eq(agents.isDeleted, false)))
        .orderBy(desc(agents.createdAt))
}

// --- Agent Runs (read-only from omni-web, written by omni-ai) ---

export async function listAgentRuns(agentId: string, limit = 50) {
    return db
        .select()
        .from(agentRuns)
        .where(eq(agentRuns.agentId, agentId))
        .orderBy(desc(agentRuns.createdAt))
        .limit(limit)
}

export async function getAgentRun(runId: string) {
    const [run] = await db.select().from(agentRuns).where(eq(agentRuns.id, runId)).limit(1)
    return run || null
}
