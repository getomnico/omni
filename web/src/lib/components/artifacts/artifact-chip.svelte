<script lang="ts">
    import { Download } from '@lucide/svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'
    import { artifactKind } from '$lib/utils/artifacts'
    import { cn } from '$lib/utils'
    import ArtifactKindIcon from './artifact-kind-icon.svelte'

    type Props = {
        artifact: ArtifactData
        // True while this artifact is displayed in the side pane.
        isActive?: boolean
        // Briefly draws attention to a freshly presented artifact.
        emphasize?: boolean
        // When provided, the whole chip toggles the artifact pane.
        onOpen?: (artifact: ArtifactData) => void
    }

    let { artifact, isActive = false, emphasize = false, onOpen }: Props = $props()

    let kind = $derived(artifactKind(artifact.content_type, artifact.url))

    const chipClasses = cn(
        'border-border text-foreground inline-flex max-w-full items-center gap-1.5 rounded-lg border py-1 pr-2 pl-2 text-sm',
        onOpen
            ? isActive
                ? 'border-primary/60 bg-primary/5'
                : 'hover:bg-muted/60'
            : 'hover:bg-muted/60',
        emphasize && 'artifact-chip-emphasize',
    )
</script>

{#if onOpen}
    <!-- The whole chip is the toggle; download lives in the opened pane header. -->
    <button
        type="button"
        title={isActive ? 'Close artifact viewer' : 'Open in artifact viewer'}
        aria-label={isActive ? 'Close artifact viewer' : 'Open in artifact viewer'}
        aria-pressed={isActive}
        class={cn(chipClasses, 'cursor-pointer')}
        onclick={() => onOpen(artifact)}>
        <ArtifactKindIcon {kind} className="h-4 w-4 shrink-0" />
        <span class="max-w-72 min-w-0 truncate" title={artifact.title}>
            {artifact.title}
        </span>
    </button>
{:else}
    <!-- No pane available (agent run transcript): download the file instead. -->
    <a
        href={artifact.url}
        download
        rel="external"
        title="Download file"
        aria-label={`Download ${artifact.title}`}
        class={cn(chipClasses, 'cursor-pointer no-underline')}>
        <ArtifactKindIcon {kind} className="h-4 w-4 shrink-0" />
        <span class="max-w-72 min-w-0 truncate" title={artifact.title}>
            {artifact.title}
        </span>
        <Download class="text-muted-foreground h-4 w-4 shrink-0" />
    </a>
{/if}

<style>
    .artifact-chip-emphasize {
        animation: artifact-emphasize 1.8s ease-out;
    }

    @keyframes artifact-emphasize {
        0% {
            box-shadow: 0 0 0 0 rgb(139 92 246 / 0.35);
        }
        70% {
            box-shadow: 0 0 0 9px rgb(139 92 246 / 0);
        }
        100% {
            box-shadow: 0 0 0 0 rgb(139 92 246 / 0);
        }
    }
</style>
