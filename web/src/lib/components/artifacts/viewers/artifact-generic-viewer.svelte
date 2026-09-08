<script lang="ts">
    import { Download } from '@lucide/svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'
    import ArtifactKindIcon from '../artifact-kind-icon.svelte'
    import { artifactKind } from '$lib/utils/artifacts'

    type Props = {
        artifact: ArtifactData
    }

    let { artifact }: Props = $props()

    let kind = $derived(artifactKind(artifact.content_type, artifact.url))
</script>

<div class="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
    <ArtifactKindIcon {kind} className="text-muted-foreground h-10 w-10" />
    <p class="text-muted-foreground max-w-sm text-sm">
        No preview is available for this file type.
    </p>
    <a
        href={artifact.url}
        download
        rel="external"
        class="text-primary hover:text-primary/80 inline-flex cursor-pointer items-center gap-1.5 text-sm font-medium no-underline">
        <Download class="h-4 w-4" />
        Download file
    </a>
</div>
