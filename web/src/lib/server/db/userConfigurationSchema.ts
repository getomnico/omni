import type { UserConfiguration, UserMemoryMode } from '$lib/types/configuration'

export type { UserConfiguration, UserMemoryMode } from '$lib/types/configuration'

export const DEFAULT_TIMEZONE = 'UTC'

export const USER_CONFIGURATION_KEYS = {
    MEMORY_MODE: 'memory_mode',
    TIMEZONE: 'timezone',
} as const

export type UserConfigurationKey =
    (typeof USER_CONFIGURATION_KEYS)[keyof typeof USER_CONFIGURATION_KEYS]

const USER_MEMORY_MODES = new Set<UserMemoryMode>(['off', 'chat', 'full'])
const TIMEZONE_ALIASES: Record<string, string> = {
    'africa/asmera': 'Africa/Asmara',
    'africa/timbuktu': 'Africa/Bamako',
    'america/argentina/comodrivadavia': 'America/Argentina/Catamarca',
    'america/atka': 'America/Adak',
    'america/buenos_aires': 'America/Argentina/Buenos_Aires',
    'america/catamarca': 'America/Argentina/Catamarca',
    'america/coral_harbour': 'America/Atikokan',
    'america/cordoba': 'America/Argentina/Cordoba',
    'america/ensenada': 'America/Tijuana',
    'america/fort_wayne': 'America/Indiana/Indianapolis',
    'america/godthab': 'America/Nuuk',
    'america/indianapolis': 'America/Indiana/Indianapolis',
    'america/jujuy': 'America/Argentina/Jujuy',
    'america/knox_in': 'America/Indiana/Knox',
    'america/kralendijk': 'America/Curacao',
    'america/louisville': 'America/Kentucky/Louisville',
    'america/lower_princes': 'America/Curacao',
    'america/marigot': 'America/Port_of_Spain',
    'america/mendoza': 'America/Argentina/Mendoza',
    'america/montreal': 'America/Toronto',
    'america/nipigon': 'America/Toronto',
    'america/pangnirtung': 'America/Iqaluit',
    'america/porto_acre': 'America/Rio_Branco',
    'america/rainy_river': 'America/Winnipeg',
    'america/rosario': 'America/Argentina/Cordoba',
    'america/santa_isabel': 'America/Tijuana',
    'america/shiprock': 'America/Denver',
    'america/st_barthelemy': 'America/Port_of_Spain',
    'america/thunder_bay': 'America/Toronto',
    'america/virgin': 'America/St_Thomas',
    'america/yellowknife': 'America/Edmonton',
    'antarctica/south_pole': 'Antarctica/McMurdo',
    'arctic/longyearbyen': 'Europe/Oslo',
    'asia/ashkhabad': 'Asia/Ashgabat',
    'asia/calcutta': 'Asia/Kolkata',
    'asia/choibalsan': 'Asia/Ulaanbaatar',
    'asia/chongqing': 'Asia/Shanghai',
    'asia/chungking': 'Asia/Shanghai',
    'asia/dacca': 'Asia/Dhaka',
    'asia/harbin': 'Asia/Shanghai',
    'asia/istanbul': 'Europe/Istanbul',
    'asia/kashgar': 'Asia/Urumqi',
    'asia/katmandu': 'Asia/Kathmandu',
    'asia/macao': 'Asia/Macau',
    'asia/rangoon': 'Asia/Yangon',
    'asia/saigon': 'Asia/Ho_Chi_Minh',
    'asia/tel_aviv': 'Asia/Jerusalem',
    'asia/thimbu': 'Asia/Thimphu',
    'asia/ujung_pandang': 'Asia/Makassar',
    'asia/ulan_bator': 'Asia/Ulaanbaatar',
    'atlantic/faeroe': 'Atlantic/Faroe',
    'atlantic/jan_mayen': 'Europe/Oslo',
    'australia/act': 'Australia/Sydney',
    'australia/canberra': 'Australia/Sydney',
    'australia/currie': 'Australia/Hobart',
    'australia/lhi': 'Australia/Lord_Howe',
    'australia/north': 'Australia/Darwin',
    'australia/nsw': 'Australia/Sydney',
    'australia/queensland': 'Australia/Brisbane',
    'australia/south': 'Australia/Adelaide',
    'australia/tasmania': 'Australia/Hobart',
    'australia/victoria': 'Australia/Melbourne',
    'australia/west': 'Australia/Perth',
    'australia/yancowinna': 'Australia/Broken_Hill',
    'brazil/acre': 'America/Rio_Branco',
    'brazil/denoronha': 'America/Noronha',
    'brazil/east': 'America/Sao_Paulo',
    'brazil/west': 'America/Manaus',
    'canada/atlantic': 'America/Halifax',
    'canada/central': 'America/Winnipeg',
    'canada/eastern': 'America/Toronto',
    'canada/mountain': 'America/Edmonton',
    'canada/newfoundland': 'America/St_Johns',
    'canada/pacific': 'America/Vancouver',
    'canada/saskatchewan': 'America/Regina',
    'canada/yukon': 'America/Whitehorse',
    'chile/continental': 'America/Santiago',
    'chile/easterisland': 'Pacific/Easter',
    cuba: 'America/Havana',
    egypt: 'Africa/Cairo',
    eire: 'Europe/Dublin',
    'etc/gmt+0': 'Etc/GMT',
    'etc/gmt-0': 'Etc/GMT',
    'etc/gmt0': 'Etc/GMT',
    'etc/greenwich': 'Etc/GMT',
    'etc/uct': 'Etc/UTC',
    'etc/universal': 'Etc/UTC',
    'etc/zulu': 'Etc/UTC',
    'europe/belfast': 'Europe/London',
    'europe/bratislava': 'Europe/Prague',
    'europe/busingen': 'Europe/Zurich',
    'europe/kiev': 'Europe/Kyiv',
    'europe/mariehamn': 'Europe/Helsinki',
    'europe/nicosia': 'Asia/Nicosia',
    'europe/podgorica': 'Europe/Belgrade',
    'europe/san_marino': 'Europe/Rome',
    'europe/tiraspol': 'Europe/Chisinau',
    'europe/uzhgorod': 'Europe/Kyiv',
    'europe/vatican': 'Europe/Rome',
    'europe/zaporozhye': 'Europe/Kyiv',
    gb: 'Europe/London',
    'gb-eire': 'Europe/London',
    gmt: 'Etc/GMT',
    'gmt+0': 'Etc/GMT',
    'gmt-0': 'Etc/GMT',
    gmt0: 'Etc/GMT',
    greenwich: 'Etc/GMT',
    hongkong: 'Asia/Hong_Kong',
    iceland: 'Atlantic/Reykjavik',
    iran: 'Asia/Tehran',
    israel: 'Asia/Jerusalem',
    jamaica: 'America/Jamaica',
    japan: 'Asia/Tokyo',
    kwajalein: 'Pacific/Kwajalein',
    libya: 'Africa/Tripoli',
    'mexico/bajanorte': 'America/Tijuana',
    'mexico/bajasur': 'America/Mazatlan',
    'mexico/general': 'America/Mexico_City',
    navajo: 'America/Denver',
    nz: 'Pacific/Auckland',
    'nz-chat': 'Pacific/Chatham',
    'pacific/enderbury': 'Pacific/Kanton',
    'pacific/johnston': 'Pacific/Honolulu',
    'pacific/ponape': 'Pacific/Pohnpei',
    'pacific/samoa': 'Pacific/Pago_Pago',
    'pacific/truk': 'Pacific/Chuuk',
    'pacific/yap': 'Pacific/Chuuk',
    poland: 'Europe/Warsaw',
    portugal: 'Europe/Lisbon',
    prc: 'Asia/Shanghai',
    roc: 'Asia/Taipei',
    rok: 'Asia/Seoul',
    singapore: 'Asia/Singapore',
    turkey: 'Europe/Istanbul',
    uct: 'Etc/UTC',
    universal: 'Etc/UTC',
    'us/alaska': 'America/Anchorage',
    'us/aleutian': 'America/Adak',
    'us/arizona': 'America/Phoenix',
    'us/central': 'America/Chicago',
    'us/east-indiana': 'America/Indiana/Indianapolis',
    'us/eastern': 'America/New_York',
    'us/hawaii': 'Pacific/Honolulu',
    'us/indiana-starke': 'America/Indiana/Knox',
    'us/michigan': 'America/Detroit',
    'us/mountain': 'America/Denver',
    'us/pacific': 'America/Los_Angeles',
    'us/samoa': 'Pacific/Pago_Pago',
    utc: 'UTC',
    'w-su': 'Europe/Moscow',
    zulu: 'Etc/UTC',
}

function extractStringValue(raw: unknown, alternateKeys: string[] = []): string | null {
    if (typeof raw === 'string') return raw
    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
        const record = raw as Record<string, unknown>
        const candidates = ['value', ...alternateKeys]
        for (const key of candidates) {
            const value = record[key]
            if (typeof value === 'string') return value
        }
    }
    return null
}

export function normalizeTimezone(timezone: string): string | null {
    const candidate = timezone.trim()
    if (!candidate) return null

    const directAlias = TIMEZONE_ALIASES[candidate.toLowerCase()]
    if (directAlias) return directAlias

    try {
        const resolved = new Intl.DateTimeFormat('en-US', {
            timeZone: candidate,
        }).resolvedOptions().timeZone
        return TIMEZONE_ALIASES[resolved.toLowerCase()] ?? resolved
    } catch {
        return null
    }
}

export function isValidTimezone(timezone: string): boolean {
    return normalizeTimezone(timezone) !== null
}

export function extractUserTimezone(raw: unknown): string | null {
    const value = extractStringValue(raw, ['timezone'])
    if (!value) return null
    return normalizeTimezone(value)
}

export function extractUserMemoryMode(raw: unknown): UserMemoryMode | null {
    const value = extractStringValue(raw, ['mode'])
    if (!value || !USER_MEMORY_MODES.has(value as UserMemoryMode)) return null
    return value as UserMemoryMode
}

export function assertUserMemoryMode(mode: string): asserts mode is UserMemoryMode {
    if (!USER_MEMORY_MODES.has(mode as UserMemoryMode)) {
        throw new Error('Invalid memory mode')
    }
}
