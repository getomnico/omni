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
    return agent
}

export async function deleteAgent(agentId: string) {
    const existing = await getAgent(agentId)
    const [agent] = await db
        .update(agents)
        .set({ isDeleted: true, isEnabled: false, updatedAt: new Date() })
        .where(eq(agents.id, agentId))
        .returning()

    // For org agents, purge the mem0 namespace. Best-effort — never block delete.
    if (existing?.agentType === 'org') {
        try {
            const { getConfig } = await import('../config.js')
            const { services } = getConfig()
            const resp = await fetch(
                `${services.aiServiceUrl}/memories/org-agent/${encodeURIComponent(agentId)}`,
                {
                    method: 'DELETE',
                    headers: { 'x-user-id': 'system', 'x-user-role': 'admin' },
                },
            )
            if (!resp.ok && resp.status !== 503) {
                console.warn(
                    `Org-agent memory purge returned ${resp.status} for ${agentId}`,
                )
            }
        } catch (err) {
            console.warn(`Org-agent memory purge failed for ${agentId}:`, err)
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
