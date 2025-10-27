import type { PageServerLoad } from './$types.js'
import { db } from '$lib/server/db/index.js'
import { sources, documents } from '$lib/server/db/schema.js'
import { eq, sql } from 'drizzle-orm'
import { env } from '$env/dynamic/private'
import { logger } from '$lib/server/logger.js'
import type { SuggestedQuestion, SuggestedQuestionsResponse } from '$lib/types/search.js'

export const load: PageServerLoad = async ({ locals, fetch }) => {
    // Get connected sources count
    const connectedSources = await db.select().from(sources).where(eq(sources.isActive, true))
    const connectedSourcesCount = connectedSources.length

    // Get total indexed documents count
    const documentsBySource = await db
        .select({
            count: sql<number>`COUNT(*)::int`,
        })
        .from(documents)

    const totalDocumentsIndexed = documentsBySource.length > 0 ? documentsBySource[0].count : 0

    // Fetch recent searches if user is logged in
    let recentSearches: string[] = []
    if (locals.user?.id) {
        try {
            const recentResponse = await fetch('/api/search/recent')
            if (recentResponse.ok) {
                const data = await recentResponse.json()
                recentSearches = data.searches || []
            }
        } catch (error) {
            console.error('Failed to fetch recent searches:', error)
        }
    }

    // Fetch suggested questions
    let suggestedQuestions: SuggestedQuestion[] = []
    try {
        const suggestedResponse = await fetch(`${env.SEARCHER_URL}/suggested-questions`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({
                user_id: locals.user?.id,
            }),
        })

        if (suggestedResponse.ok) {
            const data: SuggestedQuestionsResponse = await suggestedResponse.json()

            // Randomly pick 3 out of the available questions
            const questions: SuggestedQuestion[] = []
            const taken = new Set()
            while (questions.length < 3 && questions.length < data.questions.length) {
                const index = Math.floor(Math.random() * data.questions.length)
                if (!taken.has(index)) {
                    taken.add(index)
                    questions.push(data.questions[index])
                }
            }
            suggestedQuestions = questions
        }
    } catch (error) {
        logger.error('Failed to fetch suggested questions', { error })
    }

    logger.info('Loaded app page data', {
        userId: locals.user?.id,
        connectedSources: connectedSourcesCount,
        indexedDocuments: totalDocumentsIndexed,
        recentSearchesCount: recentSearches.length,
        suggestedQuestionsCount: suggestedQuestions.length,
        aiFirstSearchEnabled: env.AI_FIRST_SEARCH_ENABLED === 'true',
    })

    return {
        user: locals.user!,
        stats: {
            connectedSources: connectedSourcesCount,
            indexedDocuments: totalDocumentsIndexed,
        },
        recentSearches,
        suggestedQuestions,
        aiFirstSearchEnabled: env.AI_FIRST_SEARCH_ENABLED === 'true',
    }
}
