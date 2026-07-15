import { describe, expect, it } from 'vitest'
import { createSkillSchema, updateSkillSchema } from './skills'

describe('skill payload schemas', () => {
    it('trims valid create payloads', () => {
        const parsed = createSkillSchema.parse({
            name: '  PR Review  ',
            instructions: '  Review carefully.  ',
            visibility: 'public',
        })

        expect(parsed).toEqual({
            name: 'PR Review',
            instructions: 'Review carefully.',
            visibility: 'public',
        })
    })

    it('rejects whitespace-only create and update payloads', () => {
        expect(createSkillSchema.safeParse({ name: '   ', instructions: 'Do it.' }).success).toBe(
            false,
        )
        expect(createSkillSchema.safeParse({ name: 'Name', instructions: '   ' }).success).toBe(
            false,
        )
        expect(updateSkillSchema.safeParse({ instructions: '   ' }).success).toBe(false)
    })

    it('rejects unknown visibility and empty updates', () => {
        expect(
            createSkillSchema.safeParse({
                name: 'Name',
                instructions: 'Do it.',
                visibility: 'team',
            }).success,
        ).toBe(false)
        expect(updateSkillSchema.safeParse({}).success).toBe(false)
    })
})
