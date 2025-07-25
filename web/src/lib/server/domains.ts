import { eq } from 'drizzle-orm'
import { db } from './db'
import { approvedDomains, user, type ApprovedDomain } from './db/schema'
import { createId } from '@paralleldrive/cuid2'

export interface DomainResult {
    success: boolean
    domain?: ApprovedDomain
    error?: string
}

export interface DomainListResult {
    success: boolean
    domains?: ApprovedDomain[]
    error?: string
}

export class DomainService {
    static async isDomainApproved(domain: string): Promise<boolean> {
        try {
            const result = await db
                .select({ id: approvedDomains.id })
                .from(approvedDomains)
                .where(eq(approvedDomains.domain, domain))
                .limit(1)

            return result.length > 0
        } catch (error) {
            console.error('Error checking domain approval:', error)
            return false
        }
    }

    static async approveDomain(domain: string, approvedBy: string): Promise<DomainResult> {
        try {
            // Check if domain is already approved
            const existing = await db
                .select()
                .from(approvedDomains)
                .where(eq(approvedDomains.domain, domain))
                .limit(1)

            if (existing.length > 0) {
                return {
                    success: false,
                    error: 'Domain is already approved',
                }
            }

            // Verify the approver is an admin
            const approver = await db
                .select({ role: user.role })
                .from(user)
                .where(eq(user.id, approvedBy))
                .limit(1)

            if (approver.length === 0 || approver[0].role !== 'admin') {
                return {
                    success: false,
                    error: 'Only admins can approve domains',
                }
            }

            const id = createId()
            const newDomain = {
                id,
                domain,
                approvedBy,
                createdAt: new Date(),
                updatedAt: new Date(),
            }

            await db.insert(approvedDomains).values(newDomain)

            return {
                success: true,
                domain: newDomain,
            }
        } catch (error) {
            console.error('Error approving domain:', error)
            return {
                success: false,
                error: 'Failed to approve domain',
            }
        }
    }

    static async revokeDomain(domain: string, revokedBy: string): Promise<DomainResult> {
        try {
            // Verify the revoker is an admin
            const revoker = await db
                .select({ role: user.role })
                .from(user)
                .where(eq(user.id, revokedBy))
                .limit(1)

            if (revoker.length === 0 || revoker[0].role !== 'admin') {
                return {
                    success: false,
                    error: 'Only admins can revoke domains',
                }
            }

            const result = await db
                .delete(approvedDomains)
                .where(eq(approvedDomains.domain, domain))
                .returning()

            if (result.length === 0) {
                return {
                    success: false,
                    error: 'Domain not found',
                }
            }

            return {
                success: true,
                domain: result[0],
            }
        } catch (error) {
            console.error('Error revoking domain:', error)
            return {
                success: false,
                error: 'Failed to revoke domain',
            }
        }
    }

    static async getApprovedDomains(): Promise<DomainListResult> {
        try {
            const domains = await db
                .select()
                .from(approvedDomains)
                .orderBy(approvedDomains.createdAt)

            return {
                success: true,
                domains,
            }
        } catch (error) {
            console.error('Error getting approved domains:', error)
            return {
                success: false,
                error: 'Failed to get approved domains',
            }
        }
    }

    static extractDomainFromEmail(email: string): string | null {
        const match = email.match(/@(.+)$/)
        return match ? match[1].toLowerCase() : null
    }

    static async autoApproveDomainForAdmin(adminUserId: string): Promise<boolean> {
        try {
            // Get admin user's email
            const adminUser = await db
                .select({ email: user.email })
                .from(user)
                .where(eq(user.id, adminUserId))
                .limit(1)

            if (adminUser.length === 0) {
                return false
            }

            const domain = this.extractDomainFromEmail(adminUser[0].email)
            if (!domain) {
                return false
            }

            // Check if domain is already approved
            const isApproved = await this.isDomainApproved(domain)
            if (isApproved) {
                return true
            }

            // Auto-approve the domain
            const result = await this.approveDomain(domain, adminUserId)
            return result.success
        } catch (error) {
            console.error('Error auto-approving domain for admin:', error)
            return false
        }
    }
}

export async function isEmailFromApprovedDomain(email: string): Promise<boolean> {
    const domain = DomainService.extractDomainFromEmail(email)
    if (!domain) {
        return false
    }
    return await DomainService.isDomainApproved(domain)
}
