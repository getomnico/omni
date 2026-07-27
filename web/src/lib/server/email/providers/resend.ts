import { Resend } from 'resend'
import { EmailProvider, type EmailResult, type SendEmailParams } from '../types'
import { createLogger } from '../../logger.js'

const logger = createLogger('resend-email')

export class ResendEmailProvider extends EmailProvider {
    private resend: Resend
    private fromEmail: string

    constructor(apiKey: string, fromEmail: string) {
        super()
        this.resend = new Resend(apiKey)
        this.fromEmail = fromEmail
    }

    async send(params: SendEmailParams): Promise<EmailResult> {
        try {
            const { data, error } = await this.resend.emails.send({
                from: this.fromEmail,
                to: [params.to],
                subject: params.subject,
                html: params.html,
            })

            if (error) {
                logger.error('Resend error', error)
                return { success: false, error: error.message || 'Failed to send email' }
            }

            return { success: true, messageId: data?.id }
        } catch (error) {
            logger.error('Error sending email via Resend', error)
            return { success: false, error: 'Failed to send email' }
        }
    }

    async testConnection(): Promise<boolean> {
        try {
            const { error } = await this.resend.domains.list()
            return !error
        } catch (error) {
            logger.error('Resend connection test failed', error)
            return false
        }
    }
}
