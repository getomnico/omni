export type UserMemoryMode = 'off' | 'chat' | 'full'

export interface UserConfiguration {
    memoryMode: UserMemoryMode | null
    timezone: string | null
}
