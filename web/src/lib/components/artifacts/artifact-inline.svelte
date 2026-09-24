<script lang="ts">
    import { Download } from '@lucide/svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'
    import { artifactKind } from '$lib/utils/artifacts'

    type Props = {
        artifact: ArtifactData
    }

    let { artifact }: Props = $props()

    let kind = $derived(artifactKind(artifact.content_type, artifact.url))
    let frameHeight = $derived(artifact.inline_height ?? 640)
</script>

{#if kind === 'html'}
    <div class="border-muted-foreground/30 w-full overflow-hidden rounded-lg border border-dotted">
        <div class="flex h-6 items-center justify-end">
            <a
                href={artifact.url}
                download
                rel="external"
                title="Download component"
                aria-label={`Download ${artifact.title}`}
                class="text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer rounded p-1">
                <Download class="h-3.5 w-3.5" />
            </a>
        </div>
        <!-- No allow-same-origin token: generated code gets an opaque origin. -->
        <iframe
            title={artifact.title}
            src={artifact.url}
            sandbox="allow-scripts allow-downloads"
            class="block w-full border-0 bg-transparent"
            style:height={`${frameHeight}px`}></iframe>
    </div>
{:else}
    <figure class="border-border overflow-hidden rounded-lg border">
        <div class="p-2 pb-0">
            <img src={artifact.url} alt={artifact.title} class="!m-0 max-w-full rounded" />
        </div>
        <figcaption class="text-muted-foreground p-2 text-center text-xs">
            {artifact.title}
        </figcaption>
    </figure>
{/if}
