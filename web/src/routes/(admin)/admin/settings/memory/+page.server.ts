import { error, fail } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    GLOBAL_CONFIGURATION_KEYS,
    getTypedGlobal,
    setTypedGlobal,
    type MemoryMode,
} from '$lib/server/db/configuration'
import { listAllActiveModels } from '$lib/server/db/model-providers'
import { getCurrentProvider } from '$lib/server/db/embedding-providers'
import type { PageServerLoad, Actions } from './$types'

const VALID_MODES = ['off', 'chat', 'full'] as const

function isMemoryMode(mode: string): mode is MemoryMode {
    return VALID_MODES.includes(mode as MemoryMode)
}

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)
    if (env.MEMORY_ENABLED !== 'true') throw error(404)

    const [orgDefaultConfig, memoryLlmConfig, models, embedder] = await Promise.all([
        getTypedGlobal(GLOBAL_CONFIGURATION_KEYS.MEMORY_MODE_DEFAULT),
        getTypedGlobal(GLOBAL_CONFIGURATION_KEYS.MEMORY_LLM_ID),
        listAllActiveModels(),
        getCurrentProvider(),
    ])

    const orgDefault = orgDefaultConfig?.value ?? 'off'
    const memoryLlmId = memoryLlmConfig?.value ?? ''
    const embedderAvailable = embedder !== null

    return { orgDefault, memoryLlmId, models, embedderAvailable }
}

export const actions: Actions = {
    save: async ({ request, locals }) => {
        requireAdmin(locals)
        if (env.MEMORY_ENABLED !== 'true') throw error(404)

        const formData = await request.formData()
        const mode = formData.get('mode') as string
        const llmId = (formData.get('llmId') as string) ?? ''

        if (!isMemoryMode(mode)) {
            return fail(400, { error: 'Invalid memory mode' })
        }

        const embedder = await getCurrentProvider()
        if (!embedder && mode !== 'off') {
            return fail(400, {
                error: 'Configure an embedding provider in Admin → Embeddings before enabling memory.',
            })
        }

        try {
            await Promise.all([
                setTypedGlobal(GLOBAL_CONFIGURATION_KEYS.MEMORY_MODE_DEFAULT, { value: mode }),
                setTypedGlobal(GLOBAL_CONFIGURATION_KEYS.MEMORY_LLM_ID, { value: llmId }),
            ])
            return { success: true }
        } catch (err) {
            console.error('Failed to update memory settings:', err)
            return fail(500, { error: 'Failed to save settings' })
        }
    },
}
