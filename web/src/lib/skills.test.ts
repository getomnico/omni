import { describe, expect, it } from 'vitest'
import { createSkillSchema, updateSkillSchema } from './skills'

describe('skill payload schemas', () => {
    it('trims valid create payloads', () => {
        const parsed = createSkillSchema.parse({
            name: '  PR Review  ',
            description: '  Review pull requests.  ',
            instructions: '  Review carefully.  ',
            visibility: 'public',
        })

        expect(parsed).toEqual({
            name: 'PR Review',
            description: 'Review pull requests.',
            instructions: 'Review carefully.',
            visibility: 'public',
        })
    })

    it('rejects whitespace-only create and update payloads', () => {
        expect(
            createSkillSchema.safeParse({
                name: '   ',
                description: 'Do it.',
                instructions: 'Do it.',
            }).success,
        ).toBe(false)
        expect(
            createSkillSchema.safeParse({
                name: 'Name',
                description: '   ',
                instructions: 'Do it.',
            }).success,
        ).toBe(false)
        expect(
            createSkillSchema.safeParse({
                name: 'Name',
                description: 'Desc',
                instructions: '   ',
            }).success,
        ).toBe(false)
        expect(updateSkillSchema.safeParse({ description: '   ' }).success).toBe(false)
    })

    it('rejects description over 500 characters', () => {
        const longDesc = 'x'.repeat(501)
        expect(
            createSkillSchema.safeParse({
                name: 'Name',
                description: longDesc,
                instructions: 'Do it.',
            }).success,
        ).toBe(false)
        expect(updateSkillSchema.safeParse({ description: longDesc }).success).toBe(false)
    })

    it('accepts description exactly 500 characters', () => {
        const desc500 = 'x'.repeat(500)
        expect(
            createSkillSchema.safeParse({
                name: 'Name',
                description: desc500,
                instructions: 'Do it.',
            }).success,
        ).toBe(true)
    })

    it('rejects unknown visibility and empty updates', () => {
        expect(
            createSkillSchema.safeParse({
                name: 'Name',
                description: 'Desc',
                instructions: 'Do it.',
                visibility: 'team',
            }).success,
        ).toBe(false)
        expect(updateSkillSchema.safeParse({}).success).toBe(false)
    })
})
