export type MemoryMode = 'off' | 'chat' | 'full'
export type UserMemoryMode = MemoryMode
export type DoclingQualityPreset = 'fast' | 'balanced' | 'quality'

export interface GlobalConfiguration {
    doclingEnabled: boolean
    doclingQualityPreset: DoclingQualityPreset
    memoryModeDefault: MemoryMode
    memoryLlmId: string | null
}

export interface UserConfiguration {
    memoryMode: MemoryMode | null
    timezone: string | null
}
