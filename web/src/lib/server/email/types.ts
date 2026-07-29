import {
    generateMagicLinkHtml,
    generateMagicLinkText,
    generateTestEmailHtml,
    generateTestEmailText,
} from './templates'
import { createLogger } from '../logger.js'

const logger = createLogger('email')

export interface EmailResult {
    success: boolean
    messageId?: string
    error?: string
}

export interface SendEmailParams {
    to: string
    subject: string
    html: string
    text?: string
}

export abstract class EmailProvider {
    abstract send(params: SendEmailParams): Promise<EmailResult>

    abstract testConnection(): Promise<boolean>

    protected async sendAndLog(params: SendEmailParams): Promise<EmailResult> {
        logger.info('Sending email')
        const result = await this.send(params)
        if (result.success) {
            logger.info(`Email sent: messageId=${result.messageId}`)
        } else {
            logger.error('Email failed', { error: result.error })
        }
        return result
    }

    async sendMagicLink(
        email: string,
        magicLinkUrl: string,
        isNewUser: boolean = false,
    ): Promise<EmailResult> {
        const subject = isNewUser
            ? 'Welcome to Omni - Complete your account setup'
            : 'Your Omni login link'

        return this.sendAndLog({
            to: email,
            subject,
            html: generateMagicLinkHtml(magicLinkUrl, email, isNewUser),
            text: generateMagicLinkText(magicLinkUrl, email, isNewUser),
        })
    }

    async sendTestEmail(email: string): Promise<EmailResult> {
        return this.sendAndLog({
            to: email,
            subject: 'Omni - Test Email',
            html: generateTestEmailHtml(email),
            text: generateTestEmailText(email),
        })
    }
}
