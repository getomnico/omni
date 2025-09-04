import type { EmailProvider, EmailConfig } from './types'
import { ResendEmailProvider } from './providers/resend'
import { SMTPEmailProvider } from './providers/smtp'
import { env } from '$env/dynamic/private'

let emailProvider: EmailProvider | null = null

export function getEmailProvider(): EmailProvider {
    if (emailProvider) {
        return emailProvider
    }

    const config: EmailConfig = {
        provider: (env.EMAIL_PROVIDER as 'resend' | 'smtp') || 'resend',
        resendApiKey: env.RESEND_API_KEY || undefined,
        fromEmail: env.EMAIL_FROM || 'Omni <noreply@yourdomain.com>',
        smtpHost: env.EMAIL_HOST || undefined,
        smtpPort: env.EMAIL_PORT ? parseInt(env.EMAIL_PORT) : undefined,
        smtpUser: env.EMAIL_USER || undefined,
        smtpPassword: env.EMAIL_PASSWORD || undefined,
        smtpSecure: env.EMAIL_SECURE === 'true',
    }

    if (config.provider === 'resend') {
        if (!config.resendApiKey) {
            throw new Error(
                'RESEND_API_KEY environment variable is required when using Resend provider',
            )
        }
        emailProvider = new ResendEmailProvider(config.resendApiKey, config.fromEmail)
    } else if (config.provider === 'smtp') {
        if (!config.smtpHost || !config.smtpUser || !config.smtpPassword) {
            throw new Error(
                'SMTP configuration (EMAIL_HOST, EMAIL_USER, EMAIL_PASSWORD) is required when using SMTP provider',
            )
        }
        emailProvider = new SMTPEmailProvider(config)
    } else {
        throw new Error(`Unsupported email provider: ${config.provider}`)
    }

    return emailProvider
}

export function resetEmailProvider(): void {
    emailProvider = null
}
