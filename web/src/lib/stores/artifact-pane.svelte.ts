import type { ArtifactData } from '$lib/utils/artifacts'

// Shared bridge between the chat page (which owns artifact selection and pane
// open/close state) and the `(app)` layout (which renders the resizable pane
// column). Rune-based singleton, following the themeStore pattern.
class ArtifactPaneState {
    artifact = $state<ArtifactData | null>(null)
    open = $state(false)
    // The layout calls this when the pane is closed via the pane's close button
    // or Esc handled at the pane level.
    closeHandler = $state<(() => void) | null>(null)

    update(
        artifact: ArtifactData | null,
        open: boolean,
        closeHandler: (() => void) | null = null,
    ): void {
        this.artifact = artifact
        this.open = open
        this.closeHandler = closeHandler
    }

    close(): void {
        this.open = false
        this.artifact = null
        this.closeHandler = null
    }
}

export const artifactPaneState = new ArtifactPaneState()
