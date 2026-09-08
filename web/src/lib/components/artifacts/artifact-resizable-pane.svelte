<script lang="ts">
    import { onMount } from 'svelte'
    import { ResizablePane } from '$lib/components/ui/resizable'
    import ArtifactPane from './artifact-pane.svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'

    type Props = {
        artifact: ArtifactData
        // Width (%) of the pane when it opens.
        initialWidth: number
        onWidthChange: (size: number) => void
        onClose: () => void
    }

    let { artifact, initialWidth, onWidthChange, onClose }: Props = $props()

    // While `.artifact-opening` is present, the layout CSS forces the group to
    // the closed state (chat 100%, pane 0%). Removing it one frame later lets
    // the persistent flex-grow transition animate both panes to their final
    // sizes in tandem.
    let opening = $state(true)

    onMount(() => {
        requestAnimationFrame(() => {
            requestAnimationFrame(() => {
                opening = false
            })
        })
    })
</script>

<ResizablePane
    defaultSize={initialWidth}
    minSize={20}
    maxSize={70}
    onResize={(size) => onWidthChange(size)}
    class="min-w-0">
    <div class={opening ? 'artifact-opening h-full' : 'h-full'}>
        <ArtifactPane {artifact} {onClose} />
    </div>
</ResizablePane>
