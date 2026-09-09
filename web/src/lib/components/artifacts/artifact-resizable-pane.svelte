<script lang="ts">
    import { onMount } from 'svelte'
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
    // sizes in tandem. `.artifact-anim-open` stays a bit longer so the opening
    // transition runs at a slower, matching pace (see layout CSS).
    let opening = $state(true)
    let animOpen = $state(true)

    // Content width is pinned to px during the open/close animations so the
    // changing pane clips it instead of reflowing the viewer. It is unpinned
    // once the pane is settled so window resizes stay fluid.
    let pinnedWidth = $state<number | null>(null)
    // `.artifact-closing` is applied in the same flush as the width pin.
    let closingApplied = $state(false)
    let contentEl = $state<HTMLDivElement | null>(null)

    onMount(() => {
        // Pin to the pane's final open width before the slide-in starts: the
        // content stays put and the growing pane clips it.
        const paneEl = contentEl?.parentElement
        const groupEl = paneEl?.parentElement
        const groupWidth = groupEl?.getBoundingClientRect().width
        if (groupWidth) {
            pinnedWidth = Math.round((groupWidth * initialWidth) / 100)
        }

        requestAnimationFrame(() => {
            requestAnimationFrame(() => {
                opening = false
            })
        })
        setTimeout(() => {
            animOpen = false
        }, 720)
    })

    $effect(() => {
        if (closing) {
            const width = contentEl?.getBoundingClientRect().width
            if (width) pinnedWidth = Math.ceil(width)
            // Apply the closing class in the same flush: the width was just
            // measured (pre-transition), so the reverse transition starts now.
            closingApplied = true
        } else {
            closingApplied = false
            // Once the slide-in has settled, go back to fluid width.
            if (!animOpen && !opening) pinnedWidth = null
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
        class={[
            closingApplied ? 'artifact-closing' : opening ? 'artifact-opening' : '',
            animOpen ? 'artifact-anim-open' : '',
            'h-full',
        ]
            .filter(Boolean)
            .join(' ')}
        style:width={pinnedWidth ? `${pinnedWidth}px` : undefined}>
        <ArtifactPane {artifact} {onClose} />
    </div>
</ResizablePane>
