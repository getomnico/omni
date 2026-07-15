import { z } from 'zod'

export const SKILL_VISIBILITY_VALUES = ['private', 'public'] as const
export type SkillVisibility = (typeof SKILL_VISIBILITY_VALUES)[number]

export const createSkillSchema = z
    .object({
        name: z.string().trim().min(1, 'Name is required'),
        instructions: z.string().trim().min(1, 'Instructions are required'),
        visibility: z.enum(SKILL_VISIBILITY_VALUES).default('private'),
    })
    .strict()

export const updateSkillSchema = z
    .object({
        name: z.string().trim().min(1, 'Name is required').optional(),
        instructions: z.string().trim().min(1, 'Instructions are required').optional(),
        visibility: z.enum(SKILL_VISIBILITY_VALUES).optional(),
    })
    .strict()
    .refine(
        (data) => Object.keys(data).length > 0,
        'At least one field must be provided for update',
    )

export const cloneSkillSchema = z.object({}).strict()

export type CreateSkillInput = z.infer<typeof createSkillSchema>
export type UpdateSkillInput = z.infer<typeof updateSkillSchema>

export interface SkillResponse {
    id: string
    ownerId: string
    name: string
    instructions: string
    visibility: SkillVisibility
    createdAt: string
    updatedAt: string
}

export interface SkillListResponse {
    skills: SkillResponse[]
}

export interface SkillErrorResponse {
    error: string
}
