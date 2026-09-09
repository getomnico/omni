<script lang="ts">
    import { Download, ExternalLink, X } from '@lucide/svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'
    import { artifactKind, artifactKindLabel, formatArtifactSize } from '$lib/utils/artifacts'
    import ArtifactKindIcon from './artifact-kind-icon.svelte'
    import ArtifactPdfViewer from './viewers/artifact-pdf-viewer.svelte'
    import ArtifactHtmlViewer from './viewers/artifact-html-viewer.svelte'
    import ArtifactDocxViewer from './viewers/artifact-docx-viewer.svelte'
    import ArtifactXlsxViewer from './viewers/artifact-xlsx-viewer.svelte'
    import ArtifactMarkdownViewer from './viewers/artifact-markdown-viewer.svelte'
    import ArtifactTextViewer from './viewers/artifact-text-viewer.svelte'
    import ArtifactGenericViewer from './viewers/artifact-generic-viewer.svelte'

    type Props = {
        artifact: ArtifactData
        onClose: () => void
    }

    let { artifact, onClose }: Props = $props()

    let kind = $derived(artifactKind(artifact.content_type, artifact.url))
</script>

<aside
    aria-label={`Artifact viewer: ${artifact.title}`}
    class="bg-background flex h-full min-w-0 flex-col overflow-hidden">
    <header class="flex h-14 shrink-0 items-center gap-2.5 border-b px-3">
        <div class="bg-muted/60 flex h-8 w-8 shrink-0 items-center justify-center rounded-md">
            <ArtifactKindIcon {kind} className="h-4 w-4" />
        </div>
        <div class="min-w-0 flex-1">
            <h2 class="text-foreground truncate text-sm font-medium">
                {artifact.title}
            </h2>
            <p class="text-muted-foreground text-[11px]">
                {artifactKindLabel(kind)} · {formatArtifactSize(artifact.size_bytes)}
            </p>
        </div>
        <a
            href={artifact.url}
            target="_blank"
            rel="external noopener noreferrer"
            title="Open in new tab"
            aria-label="Open in new tab"
            class="text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer rounded-md p-2">
            <ExternalLink class="h-4 w-4" />
        </a>
        <a
            href={artifact.url}
            download
            rel="external"
            title="Download file"
            aria-label="Download file"
            class="text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer rounded-md p-2">
            <Download class="h-4 w-4" />
        </a>
        <button
            type="button"
            title="Close viewer"
            aria-label="Close viewer"
            class="text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer rounded-md p-2"
            onclick={onClose}>
            <X class="h-4 w-4" />
        </button>
    </header>
    <div class="min-h-0 flex-1">
        {#key artifact.key}
            <div class="h-full">
                {#if kind === 'pdf'}
                    <ArtifactPdfViewer {artifact} />
                {:else if kind === 'html'}
                    <ArtifactHtmlViewer {artifact} />
                {:else if kind === 'docx'}
                    <ArtifactDocxViewer {artifact} />
                {:else if kind === 'xlsx'}
                    <ArtifactXlsxViewer {artifact} />
                {:else if kind === 'markdown'}
                    <ArtifactMarkdownViewer {artifact} />
                {:else if kind === 'text'}
                    <ArtifactTextViewer {artifact} />
                {:else}
                    <ArtifactGenericViewer {artifact} />
                {/if}
            </div>
        {/key}
    </div>
</aside>
