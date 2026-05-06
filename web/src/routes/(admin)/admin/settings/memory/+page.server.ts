import { error, fail } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import { requireAdmin } from '$lib/server/authHelpers'
import { getGlobal, setGlobal } from '$lib/server/db/configuration'
import { listAllActiveModels } from '$lib/server/db/model-providers'
import { getCurrentProvider } from '$lib/server/db/embedding-providers'
import type { PageServerLoad, Actions } from './$types'

const VALID_MODES = ['off', 'chat', 'full']

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)
    if (env.MEMORY_ENABLED !== 'true') throw error(404)

    const [orgDefaultConfig, memoryLlmConfig, models, embedder] = await Promise.all([
        getGlobal('memory_mode_default'),
        getGlobal('memory_llm_id'),
        listAllActiveModels(),
        getCurrentProvider(),
    ])

    const orgDefault = (orgDefaultConfig?.value as string) ?? 'off'
    const memoryLlmId = (memoryLlmConfig?.value as string) ?? ''
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

        if (!VALID_MODES.includes(mode)) {
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
                setGlobal('memory_mode_default', { value: mode }),
                llmId
                    ? setGlobal('memory_llm_id', { value: llmId })
                    : setGlobal('memory_llm_id', { value: '' }),
            ])
            return { success: true }
        } catch (err) {
            console.error('Failed to update memory settings:', err)
            return fail(500, { error: 'Failed to save settings' })
        }
    },
}
