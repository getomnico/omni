import type { ArtifactData } from '$lib/utils/artifacts'

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
       needed in either component to keep them in sync). The source itself is
       $state so the derivations invalidate when the page binds/unbinds. */
    source = $state<(() => PaneSource) | null>(null)
    #closeHandler: (() => void) | null = null
    #closeFinishedHandler: (() => void) | null = null

    artifact = $derived(this.source?.().artifact ?? null)
    open = $derived(this.source?.().open ?? false)

    // The layout calls this when the pane is closed via the pane's close button
    // or Esc handled at the pane level.
    get closeHandler(): (() => void) | null {
        return this.#closeHandler
    }

    // Called by the layout once the slide-out transition has finished; the
    // page clears the displayed artifact, which unmounts the pane.
    get closeFinishedHandler(): (() => void) | null {
        return this.#closeFinishedHandler
    }

    bind(
        source: () => PaneSource,
        closeHandler: () => void,
        closeFinishedHandler: () => void,
    ): void {
        this.source = source
        this.#closeHandler = closeHandler
        this.#closeFinishedHandler = closeFinishedHandler
    }

    unbind(): void {
        this.source = null
        this.#closeHandler = null
        this.#closeFinishedHandler = null
    }
}

export const artifactPaneState = new ArtifactPaneState()
