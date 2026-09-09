import type { ArtifactData } from '$lib/utils/artifacts'

// Write-only placeholders; Svelte's reactive state proxies get attached to
// these via $derived below.
type PaneSource = {
    artifact: ArtifactData | null
    open: boolean
}

// Shared bridge between the chat page (which owns artifact selection and pane
// open/close state) and the `(app)` layout (which renders the resizable pane
// column). Rune-based singleton, following the themeStore pattern.
class ArtifactPaneState {
    /* The chat page binds a "source" getter on mount; these fields are derived
       selectors over it, so the layout sees page state reactively (no $effect
       needed in either component to keep them in sync). */
    #source: (() => PaneSource) | null = null
    #closeHandler: (() => void) | null = null

    artifact = $derived(this.#source?.().artifact ?? null)
    open = $derived(this.#source?.().open ?? false)

    // The layout calls this when the pane is closed via the pane's close button
    // or Esc handled at the pane level.
    get closeHandler(): (() => void) | null {
        return this.#closeHandler
    }

    bind(source: () => PaneSource, closeHandler: () => void): void {
        this.#source = source
        this.#closeHandler = closeHandler
    }

    unbind(): void {
        this.#source = null
        this.#closeHandler = null
    }
}

export const artifactPaneState = new ArtifactPaneState()
