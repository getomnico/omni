import { describe, expect, it, vi } from 'vitest'
import { GET as getCollection, POST as postCollection } from './+server'
import { DELETE, GET as getItem, PUT } from './[skillId]/+server'
import { POST as postClone } from './[skillId]/clone/+server'

const ownedSkill = {
    id: 'owned-skill',
    ownerId: 'user-1',
    name: 'Owned',
    instructions: 'Owner instructions.',
    visibility: 'private',
    createdAt: new Date(),
    updatedAt: new Date(),
}

const publicSkill = {
    id: 'public-skill',
    ownerId: 'user-2',
    name: 'Public',
    instructions: 'Public instructions.',
    visibility: 'public',
    createdAt: new Date(),
    updatedAt: new Date(),
}

const repo = {
    listVisible: vi.fn(async () => [ownedSkill, publicSkill]),
    getVisibleById: vi.fn(async (id: string) => {
        if (id === ownedSkill.id) return ownedSkill
        if (id === publicSkill.id) return publicSkill
        return null
    }),
    create: vi.fn(async (data) => ({ ...ownedSkill, ...data, id: 'created-skill' })),
    update: vi.fn(async (id: string, _userId: string, data) => ({ ...ownedSkill, id, ...data })),
    delete: vi.fn(async () => ownedSkill),
    clone: vi.fn(async () => ({
        ...publicSkill,
        id: 'cloned-skill',
        ownerId: 'user-1',
        visibility: 'private',
    })),
}

vi.mock('$lib/server/db/skills.js', () => ({
    SkillRepository: vi.fn(function SkillRepository() {
        return repo
    }),
}))

function locals(userId: string | null = 'user-1') {
    return userId ? { user: { id: userId } } : { user: null }
}

function jsonRequest(body: unknown) {
    return new Request('http://localhost/api/skills', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body),
    })
}

describe('skills API routes', () => {
    it('requires authentication', async () => {
        const response = await getCollection({ locals: locals(null) } as never)
        expect(response.status).toBe(401)
    })

    it('rejects whitespace-only create payloads before repository calls', async () => {
        repo.create.mockClear()
        const response = await postCollection({
            locals: locals(),
            request: jsonRequest({ name: '   ', instructions: 'Do it.' }),
        } as never)

        expect(response.status).toBe(400)
        expect(repo.create).not.toHaveBeenCalled()
    })

    it('returns 404 for invisible skill reads', async () => {
        const response = await getItem({
            locals: locals(),
            params: { skillId: 'missing-skill' },
        } as never)

        expect(response.status).toBe(404)
    })

    it('returns 403 when a visible non-owner attempts mutation', async () => {
        const updateResponse = await PUT({
            locals: locals(),
            params: { skillId: publicSkill.id },
            request: jsonRequest({ name: 'Nope' }),
        } as never)
        const deleteResponse = await DELETE({
            locals: locals(),
            params: { skillId: publicSkill.id },
        } as never)

        expect(updateResponse.status).toBe(403)
        expect(deleteResponse.status).toBe(403)
    })

    it('clones through the server-side repository', async () => {
        repo.clone.mockClear()
        const response = await postClone({
            locals: locals(),
            params: { skillId: publicSkill.id },
            request: jsonRequest({}),
        } as never)

        expect(response.status).toBe(201)
        expect(repo.clone).toHaveBeenCalledWith(publicSkill.id, 'user-1')
    })
})
