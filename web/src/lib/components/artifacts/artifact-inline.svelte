<script lang="ts">
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
    <!-- No allow-same-origin token: generated code gets an opaque origin. -->
    <iframe
        title={artifact.title}
        src={artifact.url}
        sandbox="allow-scripts allow-downloads"
        class="block w-full border-0 bg-transparent"
        style:height={`${frameHeight}px`}></iframe>
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
