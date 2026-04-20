import type { Theme, ThemeTokens } from './types'

const TOKEN_MAP: Record<keyof ThemeTokens, string> = {
    background: '--background',
    foreground: '--foreground',
    card: '--card',
    cardForeground: '--card-foreground',
    popover: '--popover',
    popoverForeground: '--popover-foreground',
    primary: '--primary',
    primaryForeground: '--primary-foreground',
    secondary: '--secondary',
    secondaryForeground: '--secondary-foreground',
    muted: '--muted',
    mutedForeground: '--muted-foreground',
    accent: '--accent',
    accentForeground: '--accent-foreground',
    destructive: '--destructive',
    destructiveForeground: '--destructive-foreground',
    warning: '--warning',
    warningForeground: '--warning-foreground',
    success: '--success',
    successForeground: '--success-foreground',
    border: '--border',
    input: '--input',
    ring: '--ring',
    chart1: '--chart-1',
    chart2: '--chart-2',
    chart3: '--chart-3',
    chart4: '--chart-4',
    chart5: '--chart-5',
    sidebar: '--sidebar',
    sidebarForeground: '--sidebar-foreground',
    sidebarPrimary: '--sidebar-primary',
    sidebarPrimaryForeground: '--sidebar-primary-foreground',
    sidebarAccent: '--sidebar-accent',
    sidebarAccentForeground: '--sidebar-accent-foreground',
    sidebarBorder: '--sidebar-border',
    sidebarRing: '--sidebar-ring',
    omniBrand: '--omni-brand',
    omniBrandForeground: '--omni-brand-foreground',
    radius: '--radius',
}

export function applyTheme(theme: Theme): void {
    if (typeof document === 'undefined') return
    const root = document.documentElement
    const tokens = theme.tokens

    for (const [key, cssVar] of Object.entries(TOKEN_MAP)) {
        root.style.setProperty(cssVar, tokens[key as keyof ThemeTokens])
    }

    root.setAttribute('data-theme', theme.id)

    if (theme.colorScheme === 'dark') {
        root.classList.add('dark')
    } else {
        root.classList.remove('dark')
    }
}
