import { fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    listActiveProviders,
    getProvider,
    createProvider,
    updateProvider,
    deleteProvider,
    setCurrentProvider,
    EMAIL_PROVIDER_TYPES,
    EMAIL_PROVIDER_LABELS,
    type EmailProviderConfig,
    type EmailProviderType,
} from '$lib/server/db/email-providers'
import { getEmailProvider, resetEmailProvider } from '$lib/server/email/factory'
import { ResendEmailProvider } from '$lib/server/email/providers/resend'
import { SMTPEmailProvider } from '$lib/server/email/providers/smtp'
import { ACSEmailProvider } from '$lib/server/email/providers/acs'

function stripSecrets(config: Record<string, unknown>): Record<string, unknown> {
    const { connectionString, apiKey, password, ...rest } = config
    return rest
}

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const providers = await listActiveProviders()

    return {
        adminEmail: locals.user?.email ?? null,
        providers: providers.map((p) => {
            const config = p.config as Record<string, unknown>
            return {
                id: p.id,
                name: p.name,
                providerType: p.providerType,
                config: stripSecrets(config),
                hasSecret: !!(config.connectionString || config.apiKey || config.password),
                isCurrent: p.isCurrent,
            }
        }),
    }
}

export const actions: Actions = {
    add: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const providerType = formData.get('providerType') as EmailProviderType

        if (!providerType || !EMAIL_PROVIDER_TYPES.includes(providerType))
            return fail(400, { error: 'Invalid provider type' })

        const config = parseConfig(formData, providerType)
        const validation = validateConfig(providerType, config)
        if (validation) return fail(400, { error: validation })

        const name = EMAIL_PROVIDER_LABELS[providerType] || providerType

        try {
            await createProvider({ name, providerType, config })
            resetEmailProvider()
            return { success: true, message: 'Email provider connected' }
        } catch (err) {
            console.error('Failed to add email provider:', err)
            return fail(500, { error: 'Failed to add provider' })
        }
    },

    edit: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })

        const existing = await getProvider(id)
        if (!existing) return fail(404, { error: 'Provider not found' })

        const providerType = existing.providerType as EmailProviderType
        const config = parseConfig(formData, providerType)
        const existingConfig = existing.config as Record<string, unknown>

        if (!config.connectionString && existingConfig.connectionString) {
            config.connectionString = existingConfig.connectionString as string
        }
        if (!config.apiKey && existingConfig.apiKey) {
            config.apiKey = existingConfig.apiKey as string
        }
        if (!config.password && existingConfig.password) {
            config.password = existingConfig.password as string
        }

        const validation = validateConfig(providerType, config, true)
        if (validation) return fail(400, { error: validation })

        try {
            await updateProvider(id, { config })
            resetEmailProvider()
            return { success: true, message: 'Provider updated' }
        } catch (err) {
            console.error('Failed to update email provider:', err)
            return fail(500, { error: 'Failed to update provider' })
        }
    },

    delete: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })

        try {
            await deleteProvider(id)
            resetEmailProvider()
            return { success: true, message: 'Provider removed' }
        } catch (err) {
            console.error('Failed to delete email provider:', err)
            return fail(500, { error: 'Failed to delete provider' })
        }
    },

    setCurrent: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })

        try {
            await setCurrentProvider(id)
            resetEmailProvider()
            return { success: true, message: 'Email provider switched' }
        } catch (err) {
            console.error('Failed to set current email provider:', err)
            return fail(500, { error: 'Failed to switch provider' })
        }
    },

    sendTest: async ({ locals }) => {
        requireAdmin(locals)

        const adminEmail = locals.user?.email
        if (!adminEmail) return fail(400, { error: 'Could not determine your email address' })

        resetEmailProvider()
        const provider = await getEmailProvider()
        if (!provider) return fail(400, { error: 'No email provider configured' })

        try {
            const result = await provider.sendMagicLink(
                adminEmail,
                'https://example.com/test',
                false,
            )
            if (result.success) {
                return { success: true, message: `Test email sent to ${adminEmail}` }
            }
            return fail(500, { error: result.error || 'Failed to send test email' })
        } catch (err) {
            console.error('Failed to send test email:', err)
            return fail(500, { error: 'Failed to send test email' })
        }
    },

    testConnection: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const providerType = formData.get('providerType') as EmailProviderType
        const id = formData.get('id') as string

        try {
            let provider
            if (id) {
                const dbProvider = await getProvider(id)
                if (!dbProvider) return fail(404, { error: 'Provider not found' })
                const config = dbProvider.config as EmailProviderConfig
                provider = instantiateProvider(dbProvider.providerType as EmailProviderType, config)
            } else {
                const config = parseConfig(formData, providerType)
                provider = instantiateProvider(providerType, config)
            }

            if (!provider) return fail(400, { error: 'Could not create provider instance' })

            const connected = await provider.testConnection()
            if (connected) {
                return { success: true, message: 'Connection successful' }
            } else {
                return fail(400, { error: 'Connection failed. Please check your credentials.' })
            }
        } catch (err) {
            console.error('Email connection test failed:', err)
            return fail(500, { error: 'Connection test failed' })
        }
    },
}

function instantiateProvider(providerType: EmailProviderType, config: EmailProviderConfig) {
    if (providerType === 'acs' && config.connectionString && config.senderAddress) {
        return new ACSEmailProvider(config.connectionString, config.senderAddress)
    } else if (providerType === 'resend' && config.apiKey && config.fromEmail) {
        return new ResendEmailProvider(config.apiKey, config.fromEmail)
    } else if (
        providerType === 'smtp' &&
        config.host &&
        config.user &&
        config.password &&
        config.fromEmail
    ) {
        return new SMTPEmailProvider({
            host: config.host,
            port: config.port || undefined,
            user: config.user,
            password: config.password,
            secure: config.secure || undefined,
            fromEmail: config.fromEmail,
        })
    }
    return null
}

function parseConfig(formData: FormData, providerType: string): EmailProviderConfig {
    if (providerType === 'acs') {
        return {
            connectionString: (formData.get('connectionString') as string) || null,
            senderAddress: (formData.get('senderAddress') as string)?.trim() || null,
        }
    } else if (providerType === 'resend') {
        return {
            apiKey: (formData.get('apiKey') as string) || null,
            fromEmail: (formData.get('fromEmail') as string)?.trim() || null,
        }
    } else {
        const portStr = formData.get('port') as string
        return {
            host: (formData.get('host') as string)?.trim() || null,
            port: portStr ? parseInt(portStr, 10) : null,
            user: (formData.get('user') as string) || null,
            password: (formData.get('password') as string) || null,
            secure: formData.get('secure') === 'true',
            fromEmail: (formData.get('fromEmail') as string)?.trim() || null,
        }
    }
}

function validateConfig(
    providerType: string,
    config: EmailProviderConfig,
    isEdit = false,
): string | null {
    if (providerType === 'acs') {
        if (!config.connectionString && !isEdit) return 'Connection string is required'
        if (!config.senderAddress) return 'Sender address is required'
    } else if (providerType === 'resend') {
        if (!config.apiKey && !isEdit) return 'API key is required'
        if (!config.fromEmail) return 'From email is required'
    } else if (providerType === 'smtp') {
        if (!config.host) return 'SMTP host is required'
        if (!config.user && !isEdit) return 'SMTP username is required'
        if (!config.password && !isEdit) return 'SMTP password is required'
        if (!config.fromEmail) return 'From email is required'
    }
    return null
}
