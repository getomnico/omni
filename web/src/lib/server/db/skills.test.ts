import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { eq } from 'drizzle-orm'
import { startTestDb, stopTestDb, createTestUser } from './test-setup'
import { SkillRepository } from './skills'
import * as schema from './schema'

let db: PostgresJsDatabase<typeof schema>
let repo: SkillRepository
let user1Id: string
let user2Id: string

beforeAll(async () => {
    db = await startTestDb()
    repo = new SkillRepository(db)
})

afterAll(async () => {
    await stopTestDb()
})

beforeEach(async () => {
    // Clean up any skills left by previous tests
    await db.delete(schema.skills)
    user1Id = await createTestUser(db)
    user2Id = await createTestUser(db)
})

describe('SkillRepository', () => {
    it('creates a skill with default private visibility', async () => {
        const skill = await repo.create({
            userId: user1Id,
            name: 'PR Review',
            description: 'Review pull requests for code quality.',
            instructions: 'Review pull requests for code quality.',
        })

        expect(skill).toBeDefined()
        expect(skill.ownerId).toBe(user1Id)
        expect(skill.name).toBe('PR Review')
        expect(skill.description).toBe('Review pull requests for code quality.')
        expect(skill.instructions).toBe('Review pull requests for code quality.')
        expect(skill.visibility).toBe('private')
        expect(skill.id).toBeTruthy()
        expect(skill.createdAt).toBeInstanceOf(Date)
        expect(skill.updatedAt).toBeInstanceOf(Date)
    })

    it('creates a skill with public visibility', async () => {
        const skill = await repo.create({
            userId: user1Id,
            name: 'Public PR Review',
            description: 'Review pull requests.',
            instructions: 'Review pull requests.',
            visibility: 'public',
        })

        expect(skill.visibility).toBe('public')
    })

    it('listVisible returns own private and all public skills', async () => {
        // user1 creates a private skill
        await repo.create({
            userId: user1Id,
            name: 'Private Skill',
            description: 'Only user1 can see this.',
            instructions: 'Only user1 can see this.',
        })

        // user1 creates a public skill
        await repo.create({
            userId: user1Id,
            name: 'Public Skill',
            description: 'Everyone can see this.',
            instructions: 'Everyone can see this.',
            visibility: 'public',
        })

        // user2 creates a public skill
        await repo.create({
            userId: user2Id,
            name: 'User2 Public',
            description: 'Also public.',
            instructions: 'Also public.',
            visibility: 'public',
        })

        // user2 creates a private skill
        await repo.create({
            userId: user2Id,
            name: 'User2 Private',
            description: 'Should be invisible to user1.',
            instructions: 'Should be invisible to user1.',
        })

        // user1 sees: own private, own public, user2's public (but NOT user2's private)
        const visible = await repo.listVisible(user1Id)
        const names = visible.map((s) => s.name).sort()
        expect(names).toEqual(['Private Skill', 'Public Skill', 'User2 Public'])
    })

    it('getVisibleById respects visibility', async () => {
        const privateSkill = await repo.create({
            userId: user1Id,
            name: 'Private',
            description: 'Secret.',
            instructions: 'Secret.',
        })

        const publicSkill = await repo.create({
            userId: user1Id,
            name: 'Public',
            description: 'Open.',
            instructions: 'Open.',
            visibility: 'public',
        })

        // user1 can see both
        expect(await repo.getVisibleById(privateSkill.id, user1Id)).toBeDefined()
        expect(await repo.getVisibleById(publicSkill.id, user1Id)).toBeDefined()

        // user2 can only see public
        expect(await repo.getVisibleById(privateSkill.id, user2Id)).toBeNull()
        expect(await repo.getVisibleById(publicSkill.id, user2Id)).toBeDefined()
    })

    it('update only works for the owner', async () => {
        const skill = await repo.create({
            userId: user1Id,
            name: 'My Skill',
            description: 'Original desc.',
            instructions: 'Original.',
        })

        // Non-owner cannot update
        const notUpdated = await repo.update(skill.id, user2Id, {
            name: 'Hacked',
        })
        expect(notUpdated).toBeNull()

        // Owner can update
        const updated = await repo.update(skill.id, user1Id, {
            name: 'Updated',
            description: 'New description.',
            instructions: 'New instructions.',
            visibility: 'public',
        })
        expect(updated).toBeDefined()
        expect(updated!.name).toBe('Updated')
        expect(updated!.description).toBe('New description.')
        expect(updated!.instructions).toBe('New instructions.')
        expect(updated!.visibility).toBe('public')
        expect(updated!.updatedAt.getTime()).toBeGreaterThanOrEqual(skill.updatedAt.getTime())
    })

    it('delete only works for the owner', async () => {
        const skill = await repo.create({
            userId: user1Id,
            name: 'To Delete',
            description: 'Delete me.',
            instructions: 'Delete me.',
        })

        // Non-owner cannot delete
        const notDeleted = await repo.delete(skill.id, user2Id)
        expect(notDeleted).toBeNull()

        // Owner can delete
        const deleted = await repo.delete(skill.id, user1Id)
        expect(deleted).toBeDefined()
        expect(deleted!.id).toBe(skill.id)

        // Verify it's gone
        expect(await repo.getVisibleById(skill.id, user1Id)).toBeNull()
    })

    it('clone creates an independent private copy', async () => {
        const source = await repo.create({
            userId: user1Id,
            name: 'Source Skill',
            description: 'Original description.',
            instructions: 'Original instructions.',
            visibility: 'public',
        })

        const cloned = await repo.clone(source.id, user2Id)

        expect(cloned).toBeDefined()
        expect(cloned!.id).not.toBe(source.id)
        expect(cloned!.ownerId).toBe(user2Id)
        expect(cloned!.name).toBe('Source Skill')
        expect(cloned!.description).toBe('Original description.')
        expect(cloned!.instructions).toBe('Original instructions.')
        expect(cloned!.visibility).toBe('private')

        // Cloned skill is independent: original can be deleted
        await repo.delete(source.id, user1Id)
        expect(await repo.getVisibleById(source.id, user1Id)).toBeNull()
        expect(await repo.getVisibleById(cloned!.id, user2Id)).toBeDefined()
    })

    it('clone returns null for private skills, including owned private skills', async () => {
        const privateSkill = await repo.create({
            userId: user1Id,
            name: 'Secret',
            description: 'Shhh.',
            instructions: 'Shhh.',
        })

        await expect(repo.clone(privateSkill.id, user2Id)).resolves.toBeNull()
        await expect(repo.clone(privateSkill.id, user1Id)).resolves.toBeNull()
    })

    it('allows duplicate display names across visible skills', async () => {
        const first = await repo.create({
            userId: user1Id,
            name: 'PR Review',
            description: 'First description.',
            instructions: 'First instructions.',
            visibility: 'public',
        })
        const second = await repo.create({
            userId: user2Id,
            name: 'PR Review',
            description: 'Second description.',
            instructions: 'Second instructions.',
            visibility: 'public',
        })

        const visible = await repo.listVisible(user1Id)
        const duplicates = visible.filter((skill) => skill.name === 'PR Review')
        expect(duplicates.map((skill) => skill.id).sort()).toEqual([first.id, second.id].sort())
    })

    it('database constraints reject blank names, blank instructions, blank description, and invalid visibility', async () => {
        await expect(
            repo.create({
                userId: user1Id,
                name: '   ',
                description: 'Desc.',
                instructions: 'Do it.',
            }),
        ).rejects.toThrow()
        await expect(
            repo.create({
                userId: user1Id,
                name: 'Name',
                description: '   ',
                instructions: 'Do it.',
            }),
        ).rejects.toThrow()
        await expect(
            repo.create({
                userId: user1Id,
                name: 'Name',
                description: 'Desc.',
                instructions: '   ',
            }),
        ).rejects.toThrow()
        await expect(
            repo.create({
                userId: user1Id,
                name: 'Name',
                description: 'x'.repeat(501),
                instructions: 'Do it.',
            }),
        ).rejects.toThrow()
        await expect(
            repo.create({
                userId: user1Id,
                name: 'Name',
                description: 'Desc.',
                instructions: 'Do it.',
                visibility: 'team' as 'private',
            }),
        ).rejects.toThrow()
    })

    it('deleting the owner cascades skills', async () => {
        const skill = await repo.create({
            userId: user1Id,
            name: 'Cascade',
            description: 'Owned by user1.',
            instructions: 'Owned by user1.',
        })

        await db.delete(schema.user).where(eq(schema.user.id, user1Id))

        expect(await repo.getVisibleById(skill.id, user1Id)).toBeNull()
    })

    it('database trigger updates updatedAt on mutation', async () => {
        const skill = await repo.create({
            userId: user1Id,
            name: 'Trigger',
            description: 'Original desc.',
            instructions: 'Original.',
        })

        await new Promise((resolve) => setTimeout(resolve, 10))
        const updated = await repo.update(skill.id, user1Id, { instructions: 'Updated.' })

        expect(updated).not.toBeNull()
        expect(updated!.updatedAt.getTime()).toBeGreaterThan(skill.updatedAt.getTime())
    })
})
