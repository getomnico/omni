<script lang="ts">
    import { onMount, tick } from 'svelte'
    import { ResizablePane } from '$lib/components/ui/resizable'
    import ArtifactPane from './artifact-pane.svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'

    type Props = {
        artifact: ArtifactData
        // Width (%) to animate to when the pane mounts.
        initialWidth: number
        // True while the pane slides out (reverse animation) before unmount.
        closing?: boolean
        onWidthChange: (size: number) => void
        onClose: () => void
    }

    let { artifact, initialWidth, closing = false, onWidthChange, onClose }: Props = $props()

    // While `.artifact-opening` is present, the layout CSS forces the group to
    // the closed state (chat 100%, pane 0%). Removing it one frame later lets
    // the persistent flex-grow transition animate both panes to their final
    // sizes in tandem.
    let opening = $state(true)

    // During the slide-out, the content keeps its width at close-start (pinned
    // to px) so the shrinking pane clips it instead of reflowing the viewer.
    let pinnedWidth = $state<number | null>(null)
    // `.artifact-closing` is applied one tick after the width is pinned.
    let closingApplied = $state(false)
    let contentEl = $state<HTMLDivElement | null>(null)

    onMount(() => {
        requestAnimationFrame(() => {
            requestAnimationFrame(() => {
                opening = false
            })
        })
    })

    $effect(() => {
        if (closing) {
            const width = contentEl?.getBoundingClientRect().width
            if (width) pinnedWidth = Math.ceil(width)
            tick().then(() => (closingApplied = true))
        } else {
            pinnedWidth = null
            closingApplied = false
        }
    })
</script>

<ResizablePane
    defaultSize={initialWidth}
    minSize={20}
    maxSize={70}
    onResize={(size) => onWidthChange(size)}
    class="min-w-0">
    <div
        bind:this={contentEl}
        class={closingApplied
            ? 'artifact-closing h-full'
            : opening
              ? 'artifact-opening h-full'
              : 'h-full'}
        style:width={pinnedWidth ? `${pinnedWidth}px` : undefined}>
        <ArtifactPane {artifact} {onClose} />
    </div>
</ResizablePane>
