import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { responseFeedbackRepository, type FeedbackType } from '$lib/server/db/response-feedback'

interface FeedbackRequest {
    feedbackType: FeedbackType
}

export const POST: RequestHandler = async ({ params, request, locals }) => {
    const logger = locals.logger.child('response-feedback')

    const { chatId, messageId } = params
    if (!chatId || !messageId) {
        logger.warn('Missing chatId or messageId parameter')
        return json({ error: 'chatId and messageId parameters are required' }, { status: 400 })
    }

    if (!locals.user?.id) {
        logger.warn('Attempted to submit feedback without valid user')
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    let feedbackRequest: FeedbackRequest
    try {
        feedbackRequest = await request.json()
    } catch (error) {
        logger.warn('Invalid JSON in feedback request', error)
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    if (!feedbackRequest.feedbackType) {
        logger.warn('Missing feedbackType in request')
        return json({ error: 'feedbackType is required' }, { status: 400 })
    }

    if (feedbackRequest.feedbackType !== 'upvote' && feedbackRequest.feedbackType !== 'downvote') {
        logger.warn('Invalid feedbackType value', { feedbackType: feedbackRequest.feedbackType })
        return json(
            { error: 'feedbackType must be either "upvote" or "downvote"' },
            { status: 400 },
        )
    }

    logger.debug('Submitting feedback', {
        chatId,
        messageId,
        feedbackType: feedbackRequest.feedbackType,
        userId: locals.user.id,
    })

    try {
        // Create or update feedback
        const feedback = await responseFeedbackRepository.createOrUpdate(
            messageId,
            locals.user.id,
            feedbackRequest.feedbackType,
        )

        logger.info('Feedback submitted successfully', {
            chatId,
            messageId,
            feedbackId: feedback.id,
            feedbackType: feedback.feedbackType,
            userId: locals.user.id,
        })

        return json(
            {
                feedbackId: feedback.id,
                feedbackType: feedback.feedbackType,
                status: 'success',
            },
            { status: 200 },
        )
    } catch (error) {
        logger.error('Error submitting feedback', error, { chatId, messageId })
        return json(
            {
                error: 'Failed to submit feedback',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}

export const DELETE: RequestHandler = async ({ params, locals }) => {
    const logger = locals.logger.child('response-feedback')

    const { chatId, messageId } = params
    if (!chatId || !messageId) {
        logger.warn('Missing chatId or messageId parameter')
        return json({ error: 'chatId and messageId parameters are required' }, { status: 400 })
    }

    if (!locals.user?.id) {
        logger.warn('Attempted to delete feedback without valid user')
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    logger.debug('Deleting feedback', {
        chatId,
        messageId,
        userId: locals.user.id,
    })

    try {
        const deleted = await responseFeedbackRepository.delete(messageId, locals.user.id)

        if (!deleted) {
            logger.warn('No feedback found to delete', {
                chatId,
                messageId,
                userId: locals.user.id,
            })
            return json({ error: 'No feedback found' }, { status: 404 })
        }

        logger.info('Feedback deleted successfully', {
            chatId,
            messageId,
            userId: locals.user.id,
        })

        return json(
            {
                status: 'deleted',
            },
            { status: 200 },
        )
    } catch (error) {
        logger.error('Error deleting feedback', error, { chatId, messageId })
        return json(
            {
                error: 'Failed to delete feedback',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
