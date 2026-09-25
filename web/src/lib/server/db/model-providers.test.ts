import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { eq } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { ulid } from 'ulid'
import * as schema from './schema'
import { setModelRole, type ModelRole } from './model-providers'
import { startTestDb, stopTestDb } from './test-setup'

let db: PostgresJsDatabase<typeof schema>

beforeAll(async () => {
    db = await startTestDb()
})

afterAll(async () => {
    await stopTestDb()
})

async function createModels(count: number): Promise<string[]> {
    const providerId = ulid()
    await db.insert(schema.modelProviders).values({
        id: providerId,
        name: `Role test provider ${providerId}`,
        providerType: 'openai_compatible',
        config: {},
    })

    const modelIds = Array.from({ length: count }, () => ulid())
    await db.insert(schema.models).values(
        modelIds.map((id, index) => ({
            id,
            modelProviderId: providerId,
            modelId: `role-test-${id}`,
            displayName: `Role test model ${index}`,
        })),
    )
    return modelIds
}

async function roleFor(id: string): Promise<{ isDefault: boolean; isSecondary: boolean }> {
    const [model] = await db
        .select({ isDefault: schema.models.isDefault, isSecondary: schema.models.isSecondary })
        .from(schema.models)
        .where(eq(schema.models.id, id))
        .limit(1)
    if (!model) throw new Error(`Test model ${id} was not found`)
    return model
}

async function assign(id: string, role: ModelRole): Promise<boolean> {
    return await setModelRole(id, role, db)
}

describe('setModelRole', () => {
    it('assigns default and secondary roles globally', async () => {
        const [defaultId, secondaryId] = await createModels(2)

        expect(await assign(defaultId, 'default')).toBe(true)
        expect(await assign(secondaryId, 'secondary')).toBe(true)

        expect(await roleFor(defaultId)).toEqual({ isDefault: true, isSecondary: false })
        expect(await roleFor(secondaryId)).toEqual({ isDefault: false, isSecondary: true })

        const assigned = await db
            .select({ id: schema.models.id })
            .from(schema.models)
            .where(eq(schema.models.isDefault, true))
        expect(assigned).toHaveLength(1)
        expect(assigned[0]?.id).toBe(defaultId)
    })

    it('unassigns a model without retaining either role', async () => {
        const [modelId] = await createModels(1)

        expect(await assign(modelId, 'default')).toBe(true)
        expect(await assign(modelId, 'unassigned')).toBe(true)
        expect(await roleFor(modelId)).toEqual({ isDefault: false, isSecondary: false })
    })

    it('does not clear roles when the target is invalid or deleted', async () => {
        const [liveId, deletedId] = await createModels(2)
        expect(await assign(deletedId, 'secondary')).toBe(true)
        await db
            .update(schema.models)
            .set({ isDeleted: true })
            .where(eq(schema.models.id, deletedId))

        expect(await assign(liveId, 'default')).toBe(true)
        expect(await assign(ulid(), 'secondary')).toBe(false)
        expect(await assign(deletedId, 'secondary')).toBe(false)
        expect(await roleFor(liveId)).toEqual({ isDefault: true, isSecondary: false })
    })

    it('switches a model between roles while preserving uniqueness', async () => {
        const [modelId, otherId] = await createModels(2)

        expect(await assign(modelId, 'default')).toBe(true)
        expect(await assign(modelId, 'secondary')).toBe(true)
        expect(await roleFor(modelId)).toEqual({ isDefault: false, isSecondary: true })

        expect(await assign(otherId, 'default')).toBe(true)
        expect(await roleFor(modelId)).toEqual({ isDefault: false, isSecondary: true })
        expect(await roleFor(otherId)).toEqual({ isDefault: true, isSecondary: false })
    })

    it('serializes concurrent assignments to one global role', async () => {
        const [firstId, secondId] = await createModels(2)

        const results = await Promise.all([assign(firstId, 'default'), assign(secondId, 'default')])
        expect(results).toEqual([true, true])

        const assigned = await db
            .select({ id: schema.models.id })
            .from(schema.models)
            .where(eq(schema.models.isDefault, true))
        expect(assigned).toHaveLength(1)
        expect([firstId, secondId]).toContain(assigned[0]?.id)
    })
})
