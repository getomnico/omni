import { db } from './index.js'
import { agents, agentRuns } from './schema.js'
import { eq, and, desc } from 'drizzle-orm'
import { ulid } from 'ulid'

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
    const [agent] = await db
        .update(agents)
        .set({ isDeleted: true, isEnabled: false, updatedAt: new Date() })
        .where(eq(agents.id, agentId))
        .returning()
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
