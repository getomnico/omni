import { fail } from '@sveltejs/kit'
import { DomainService } from '$lib/server/domains'
import { requireAdmin } from '$lib/server/authHelpers'
import { z } from 'zod'
import type { Actions, PageServerLoad } from './$types'

const domainActionSchema = z.object({
    domain: z
        .string()
        .min(1, 'Domain is required')
        .regex(
            /^[a-zA-Z0-9][a-zA-Z0-9-]{0,61}[a-zA-Z0-9]?(\.[a-zA-Z0-9][a-zA-Z0-9-]{0,61}[a-zA-Z0-9]?)*$/,
            'Please enter a valid domain',
        ),
})

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const domainsResult = await DomainService.getApprovedDomains()

    return {
        domains: domainsResult.success ? domainsResult.domains : [],
        error: domainsResult.success ? null : domainsResult.error,
    }
}

export const actions: Actions = {
    approve: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const domain = formData.get('domain')?.toString()?.toLowerCase()

        const validation = domainActionSchema.safeParse({ domain })
        if (!validation.success) {
            return fail(400, {
                error: validation.error.issues[0].message,
                domain,
            })
        }

        const { domain: validDomain } = validation.data

        const result = await DomainService.approveDomain(validDomain, locals.user!.id)

        if (!result.success) {
            return fail(400, {
                error: result.error,
                domain: validDomain,
            })
        }

        return {
            success: true,
            message: `Domain ${validDomain} has been approved`,
        }
    },

    revoke: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const domain = formData.get('domain')?.toString()?.toLowerCase()

        if (!domain) {
            return fail(400, {
                error: 'Domain is required',
            })
        }

        const result = await DomainService.revokeDomain(domain, locals.user!.id)

        if (!result.success) {
            return fail(400, {
                error: result.error,
            })
        }

        return {
            success: true,
            message: `Domain ${domain} has been revoked`,
        }
    },
}
