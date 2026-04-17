import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import * as schema from './schema.js'
import { agents, agentRuns } from './schema.js'
import { eq, and, desc } from 'drizzle-orm'
import { ulid } from 'ulid'
import { error } from '@sveltejs/kit'
import type { Agent } from './schema.js'
import { db } from './index'

/**
 * Fetch an agent by ID. RLS ensures the user can only see their own agents
 * (or org agents if they're an admin).
 * Throws SvelteKit error (404) on failure.
 */
export async function requireAgentAccess(
    agentId: string,
    dbInstance: PostgresJsDatabase<typeof schema> = db,
): Promise<Agent> {
    const agent = await getAgent(agentId, dbInstance)
    if (!agent) {
        throw error(404, 'Agent not found')
    }
    return agent
}

export async function createAgent(
    data: {
        userId: string
        name: string
        instructions: string
        agentType: string
        scheduleType: string
        scheduleValue: string
        modelId?: string
        allowedSources?: any[]
        allowedActions?: string[]
    },
    dbInstance: PostgresJsDatabase<typeof schema> = db,
) {
    const id = ulid()
    const [agent] = await dbInstance
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
    dbInstance: PostgresJsDatabase<typeof schema> = db,
) {
    const [agent] = await dbInstance
        .update(agents)
        .set({ ...data, updatedAt: new Date() })
        .where(and(eq(agents.id, agentId), eq(agents.isDeleted, false)))
        .returning()
    return agent
}

export async function deleteAgent(
    agentId: string,
    dbInstance: PostgresJsDatabase<typeof schema> = db,
) {
    const [agent] = await dbInstance
        .update(agents)
        .set({ isDeleted: true, isEnabled: false, updatedAt: new Date() })
        .where(eq(agents.id, agentId))
        .returning()
    return agent
}

export async function getAgent(
    agentId: string,
    dbInstance: PostgresJsDatabase<typeof schema> = db,
) {
    const [agent] = await dbInstance
        .select()
        .from(agents)
        .where(and(eq(agents.id, agentId), eq(agents.isDeleted, false)))
        .limit(1)
    return agent || null
}

export async function listAgents(dbInstance: PostgresJsDatabase<typeof schema> = db) {
    return dbInstance
        .select()
        .from(agents)
        .where(eq(agents.isDeleted, false))
        .orderBy(desc(agents.createdAt))
}

export async function listOrgAgents(dbInstance: PostgresJsDatabase<typeof schema> = db) {
    return dbInstance
        .select()
        .from(agents)
        .where(and(eq(agents.agentType, 'org'), eq(agents.isDeleted, false)))
        .orderBy(desc(agents.createdAt))
}

// --- Agent Runs (read-only from omni-web, written by omni-ai) ---

export async function listAgentRuns(
    agentId: string,
    limit = 50,
    dbInstance: PostgresJsDatabase<typeof schema> = db,
) {
    return dbInstance
        .select()
        .from(agentRuns)
        .where(eq(agentRuns.agentId, agentId))
        .orderBy(desc(agentRuns.createdAt))
        .limit(limit)
}

export async function getAgentRun(
    runId: string,
    dbInstance: PostgresJsDatabase<typeof schema> = db,
) {
    const [run] = await dbInstance.select().from(agentRuns).where(eq(agentRuns.id, runId)).limit(1)
    return run || null
}
