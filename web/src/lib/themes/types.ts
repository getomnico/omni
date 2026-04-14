/**
 * TypeScript representations of the CSS custom properties defined in app.css.
 * Token names are camelCase versions of their --kebab-case CSS counterparts.
*/

export interface ThemeTokens {
    // surfaces
    background: string
    foreground: string
    card: string
    cardForeground: string
    popover: string
    popoverForeground: string
    // interactive
    primary: string
    primaryForeground: string
    secondary: string
    secondaryForeground: string
    muted: string
    mutedForeground: string
    accent: string
    accentForeground: string
    destructive: string
    destructiveForeground: string
    // semantic status
    warning: string
    warningForeground: string
    success: string
    successForeground: string
    // form / focus
    border: string
    input: string
    ring: string
    // charts
    chart1: string
    chart2: string
    chart3: string
    chart4: string
    chart5: string
    // sidebar
    sidebar: string
    sidebarForeground: string
    sidebarPrimary: string
    sidebarPrimaryForeground: string
    sidebarAccent: string
    sidebarAccentForeground: string
    sidebarBorder: string
    sidebarRing: string
    // omni brand
    omniBrand: string
    omniBrandForeground: string
    // layout
    radius: string
}

export interface Theme {
    id: string
    name: string
    /**
     * Whether this theme is intended as a light or dark color scheme.
     * The runtime uses this to apply the appropriate Tailwind dark-mode class.
     */
    colorScheme: 'light' | 'dark'
    /** Logo URL to display when the UI is in light mode. */
    omniLogoLight: string
    /** Logo URL to display when the UI is in dark mode. */
    omniLogoDark: string
    /** Display name of the product shown next to the logo. */
    omniName: string
    tokens: ThemeTokens
}
