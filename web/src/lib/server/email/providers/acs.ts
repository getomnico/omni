import { EmailClient } from '@azure/communication-email'
import { EmailProvider, type EmailResult, type SendEmailParams } from '../types'
import { createLogger } from '../../logger.js'

const logger = createLogger('acs-email')

export class ACSEmailProvider extends EmailProvider {
    private client: EmailClient
    private senderAddress: string

    constructor(connectionString: string, senderAddress: string) {
        super()
        this.client = new EmailClient(connectionString)
        this.senderAddress = senderAddress
    }

    async send(params: SendEmailParams): Promise<EmailResult> {
        try {
            const message = {
                senderAddress: this.senderAddress,
                content: {
                    subject: params.subject,
                    html: params.html,
                    plainText: params.text,
                },
                recipients: {
                    to: [{ address: params.to }],
                },
            }

            const poller = await this.client.beginSend(message)
            const result = await poller.pollUntilDone()

            if (result.status === 'Succeeded') {
                return { success: true, messageId: result.id }
            }

            logger.error('ACS send failed', { status: result.status })
            return { success: false, error: `Email send failed with status: ${result.status}` }
        } catch (error) {
            logger.error('Error sending email via ACS', error)
            return { success: false, error: 'Failed to send email' }
        }
    }

    async testConnection(): Promise<boolean> {
        try {
            const message = {
                senderAddress: this.senderAddress,
                content: {
                    subject: 'Connection Test',
                    plainText: 'Test',
                },
                recipients: {
                    to: [{ address: 'test@test.invalid' }],
                },
            }

            const poller = await this.client.beginSend(message)
            await poller.pollUntilDone()
            return true
        } catch (error: any) {
            if (error?.code === 'Unauthorized' || error?.statusCode === 401) {
                return false
            }
            return true
        }
    }
}
