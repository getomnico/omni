import { createTransport, type Transporter } from 'nodemailer'
import type { EmailProvider, EmailResult, EmailConfig } from '../types'

export class SMTPEmailProvider implements EmailProvider {
    private transporter: Transporter
    private fromEmail: string

    constructor(config: EmailConfig) {
        this.fromEmail = config.fromEmail
        this.transporter = createTransport({
            host: config.smtpHost,
            port: config.smtpPort || 587,
            secure: config.smtpSecure || false,
            auth: {
                user: config.smtpUser,
                pass: config.smtpPassword,
            },
        })
    }

    async sendMagicLink(
        email: string,
        magicLinkUrl: string,
        isNewUser: boolean = false,
    ): Promise<EmailResult> {
        try {
            const subject = isNewUser
                ? 'Welcome to Omni - Complete your account setup'
                : 'Your Omni login link'

            const html = this.generateMagicLinkEmail(magicLinkUrl, email, isNewUser)
            const text = this.generateMagicLinkEmailText(magicLinkUrl, email, isNewUser)

            const info = await this.transporter.sendMail({
                from: this.fromEmail,
                to: email,
                subject,
                html,
                text,
            })

            return {
                success: true,
                messageId: info.messageId,
            }
        } catch (error) {
            console.error('Error sending email via SMTP:', error)
            return {
                success: false,
                error: 'Failed to send email',
            }
        }
    }

    async testConnection(): Promise<boolean> {
        try {
            await this.transporter.verify()
            return true
        } catch (error) {
            console.error('SMTP connection test failed:', error)
            return false
        }
    }

    private generateMagicLinkEmail(
        magicLinkUrl: string,
        email: string,
        isNewUser: boolean,
    ): string {
        const title = isNewUser ? 'Welcome to Omni' : 'Sign in to Omni'
        const message = isNewUser
            ? "Welcome to Omni! Click the link below to complete your account setup and access your company's search platform."
            : 'Click the link below to sign in to your Omni account.'

        return `
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>${title}</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, Cantarell, sans-serif;
            line-height: 1.6;
            color: #334155;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #f8fafc;
        }
        .container {
            background-color: white;
            padding: 40px;
            border-radius: 8px;
            box-shadow: 0 1px 3px 0 rgba(0, 0, 0, 0.1);
        }
        .header {
            text-align: center;
            margin-bottom: 30px;
        }
        .logo {
            font-size: 28px;
            font-weight: 700;
            color: #2563eb;
            margin-bottom: 8px;
        }
        .title {
            font-size: 24px;
            font-weight: 600;
            color: #1e293b;
            margin: 0;
        }
        .message {
            color: #475569;
            margin: 24px 0;
            font-size: 16px;
        }
        .button-container {
            text-align: center;
            margin: 32px 0;
        }
        .button {
            display: inline-block;
            background-color: #2563eb;
            color: white;
            padding: 16px 32px;
            text-decoration: none;
            border-radius: 8px;
            font-weight: 600;
            font-size: 16px;
            transition: background-color 0.2s;
        }
        .button:hover {
            background-color: #1d4ed8;
        }
        .footer {
            margin-top: 40px;
            padding-top: 24px;
            border-top: 1px solid #e2e8f0;
            font-size: 14px;
            color: #64748b;
        }
        .link {
            word-break: break-all;
            color: #64748b;
            font-size: 12px;
            background-color: #f1f5f9;
            padding: 8px;
            border-radius: 4px;
            margin: 16px 0;
        }
        .security-note {
            background-color: #fef3c7;
            border: 1px solid #f59e0b;
            border-radius: 6px;
            padding: 12px;
            margin: 16px 0;
            font-size: 14px;
            color: #92400e;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <div class="logo">Omni</div>
            <h1 class="title">${title}</h1>
        </div>
        
        <p class="message">Hello,</p>
        
        <p class="message">${message}</p>
        
        <div class="button-container">
            <a href="${magicLinkUrl}" class="button">
                ${isNewUser ? 'Complete Setup' : 'Sign In to Omni'}
            </a>
        </div>
        
        <div class="security-note">
            🔒 This link will expire in 15 minutes for security reasons.
        </div>
        
        <p style="font-size: 14px; color: #64748b;">
            If the button doesn't work, you can copy and paste this link into your browser:
        </p>
        <div class="link">${magicLinkUrl}</div>
        
        <div class="footer">
            <p>If you didn't request this email, you can safely ignore it.</p>
            <p>This email was sent to <strong>${email}</strong></p>
            <p>Powered by Omni - Enterprise Search Platform</p>
        </div>
    </div>
</body>
</html>
		`
    }

    private generateMagicLinkEmailText(
        magicLinkUrl: string,
        email: string,
        isNewUser: boolean,
    ): string {
        const title = isNewUser ? 'Welcome to Omni' : 'Sign in to Omni'
        const message = isNewUser
            ? "Welcome to Omni! Click the link below to complete your account setup and access your company's search platform."
            : 'Click the link below to sign in to your Omni account.'

        return `
${title}

Hello,

${message}

${isNewUser ? 'Complete Setup' : 'Sign In'}: ${magicLinkUrl}

🔒 This link will expire in 15 minutes for security reasons.

If you didn't request this email, you can safely ignore it.

This email was sent to ${email}

---
Powered by Omni - Enterprise Search Platform
		`.trim()
    }
}
