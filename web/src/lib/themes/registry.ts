import type { Theme } from './types'
import { light } from './light'
import { dark } from './dark'

export const themes: Theme[] = [light, dark]

export function getTheme(id: string): Theme {
    return themes.find((t) => t.id === id) ?? light
}
