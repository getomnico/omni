import { eq, inArray, and, desc } from 'drizzle-orm'
import { db } from './index'
import { sources, type Source } from './schema'
import type { ConfluenceSourceConfig, JiraSourceConfig, WebSourceConfig } from '$lib/types'

export type UserFilterMode = 'all' | 'whitelist' | 'blacklist'

// Generic function to get all sources of given type(s)
export async function getSourcesByType(...sourceTypes: string[]): Promise<Source[]> {
    return await db
        .select()
        .from(sources)
        .where(and(inArray(sources.sourceType, sourceTypes), eq(sources.isDeleted, false)))
        .orderBy(desc(sources.createdAt))
}

// Update a source by ID with any data
export async function updateSourceById(
    sourceId: string,
    data: {
        isActive?: boolean
        name?: string
        config?:
            | WebSourceConfig
            | ConfluenceSourceConfig
            | JiraSourceConfig
            | Record<string, unknown>
        userFilterMode?: UserFilterMode
        userWhitelist?: string[] | null
        userBlacklist?: string[] | null
    },
): Promise<void> {
    await db
        .update(sources)
        .set({
            ...data,
            updatedAt: new Date(),
        })
        .where(eq(sources.id, sourceId))
}

export interface SourceUpdateData {
    isActive?: boolean
    userFilterMode?: UserFilterMode
    userWhitelist?: string[]
    userBlacklist?: string[]
    updatedAt?: Date
}

export async function getGoogleSources(): Promise<Source[]> {
    return await db
        .select()
        .from(sources)
        .where(
            and(
                inArray(sources.sourceType, ['google_drive', 'gmail']),
                eq(sources.isDeleted, false),
            ),
        )
}

export async function getActiveGoogleSources(): Promise<Source[]> {
    return await db
        .select()
        .from(sources)
        .where(
            and(
                inArray(sources.sourceType, ['google_drive', 'gmail']),
                eq(sources.isActive, true),
                eq(sources.isDeleted, false),
            ),
        )
}

export async function updateGoogleSources(
    driveEnabled: boolean,
    gmailEnabled: boolean,
    driveSettings: {
        userFilterMode: UserFilterMode
        userWhitelist: string[] | null
        userBlacklist: string[] | null
    },
    gmailSettings: {
        userFilterMode: UserFilterMode
        userWhitelist: string[] | null
        userBlacklist: string[] | null
    },
): Promise<void> {
    await db.transaction(async (tx) => {
        // Get existing Google sources
        const googleSources = await tx
            .select()
            .from(sources)
            .where(inArray(sources.sourceType, ['google_drive', 'gmail']))

        const driveSource = googleSources.find((s) => s.sourceType === 'google_drive')
        const gmailSource = googleSources.find((s) => s.sourceType === 'gmail')

        // Update Google Drive source
        if (driveSource) {
            await tx
                .update(sources)
                .set({
                    isActive: driveEnabled,
                    userFilterMode: driveEnabled
                        ? driveSettings.userFilterMode
                        : driveSource.userFilterMode,
                    userWhitelist: driveEnabled
                        ? driveSettings.userWhitelist
                        : driveSource.userWhitelist,
                    userBlacklist: driveEnabled
                        ? driveSettings.userBlacklist
                        : driveSource.userBlacklist,
                    updatedAt: new Date(),
                })
                .where(eq(sources.id, driveSource.id))
        }

        // Update Gmail source
        if (gmailSource) {
            await tx
                .update(sources)
                .set({
                    isActive: gmailEnabled,
                    userFilterMode: gmailEnabled
                        ? gmailSettings.userFilterMode
                        : gmailSource.userFilterMode,
                    userWhitelist: gmailEnabled
                        ? gmailSettings.userWhitelist
                        : gmailSource.userWhitelist,
                    userBlacklist: gmailEnabled
                        ? gmailSettings.userBlacklist
                        : gmailSource.userBlacklist,
                    updatedAt: new Date(),
                })
                .where(eq(sources.id, gmailSource.id))
        }
    })
}

export async function updateSource(sourceId: string, data: SourceUpdateData): Promise<void> {
    const updateData: any = { ...data }

    if (data.userWhitelist) {
        updateData.userWhitelist = data.userWhitelist
    }
    if (data.userBlacklist) {
        updateData.userBlacklist = data.userBlacklist
    }

    await db.update(sources).set(updateData).where(eq(sources.id, sourceId))
}

export async function getSourceById(sourceId: string): Promise<Source | undefined> {
    const result = await db.select().from(sources).where(eq(sources.id, sourceId)).limit(1)

    return result[0]
}

export async function getAtlassianSources(): Promise<Source[]> {
    return await db
        .select()
        .from(sources)
        .where(
            and(inArray(sources.sourceType, ['confluence', 'jira']), eq(sources.isDeleted, false)),
        )
}

export async function getActiveAtlassianSources(): Promise<Source[]> {
    return await db
        .select()
        .from(sources)
        .where(
            and(
                inArray(sources.sourceType, ['confluence', 'jira']),
                eq(sources.isActive, true),
                eq(sources.isDeleted, false),
            ),
        )
}

export async function updateAtlassianSources(
    jiraEnabled: boolean,
    confluenceEnabled: boolean,
    jiraConfig?: JiraSourceConfig,
    confluenceConfig?: ConfluenceSourceConfig,
): Promise<void> {
    await db.transaction(async (tx) => {
        const atlassianSources = await tx
            .select()
            .from(sources)
            .where(inArray(sources.sourceType, ['confluence', 'jira']))

        const jiraSource = atlassianSources.find((s) => s.sourceType === 'jira')
        const confluenceSource = atlassianSources.find((s) => s.sourceType === 'confluence')

        if (jiraSource) {
            await tx
                .update(sources)
                .set({
                    isActive: jiraEnabled,
                    config: jiraConfig || jiraSource.config,
                    updatedAt: new Date(),
                })
                .where(eq(sources.id, jiraSource.id))
        }

        if (confluenceSource) {
            await tx
                .update(sources)
                .set({
                    isActive: confluenceEnabled,
                    config: confluenceConfig || confluenceSource.config,
                    updatedAt: new Date(),
                })
                .where(eq(sources.id, confluenceSource.id))
        }
    })
}

export async function getWebSources(): Promise<Source[]> {
    return await db
        .select()
        .from(sources)
        .where(and(eq(sources.sourceType, 'web'), eq(sources.isDeleted, false)))
}

export async function updateWebSource(webSettings: {
    isActive: boolean
    rootUrl?: string
    maxDepth?: number
    maxPages?: number
    respectRobotsTxt?: boolean
    includeSubdomains?: boolean
    blacklistPatterns?: string[]
    userAgent?: string | null
}): Promise<void> {
    const webSources = await db.select().from(sources).where(eq(sources.sourceType, 'web'))

    if (webSources.length > 0) {
        const webSource = webSources[0]
        const updatedConfig: any = webSource.config || {}

        if (webSettings.rootUrl !== undefined) {
            updatedConfig.root_url = webSettings.rootUrl
        }
        if (webSettings.maxDepth !== undefined) {
            updatedConfig.max_depth = webSettings.maxDepth
        }
        if (webSettings.maxPages !== undefined) {
            updatedConfig.max_pages = webSettings.maxPages
        }
        if (webSettings.respectRobotsTxt !== undefined) {
            updatedConfig.respect_robots_txt = webSettings.respectRobotsTxt
        }
        if (webSettings.includeSubdomains !== undefined) {
            updatedConfig.include_subdomains = webSettings.includeSubdomains
        }
        if (webSettings.blacklistPatterns !== undefined) {
            updatedConfig.blacklist_patterns = webSettings.blacklistPatterns
        }
        if (webSettings.userAgent !== undefined) {
            updatedConfig.user_agent = webSettings.userAgent
        }

        await db
            .update(sources)
            .set({
                isActive: webSettings.isActive,
                config: updatedConfig,
                updatedAt: new Date(),
            })
            .where(eq(sources.id, webSource.id))
    }
}
