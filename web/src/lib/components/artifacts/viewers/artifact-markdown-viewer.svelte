<script lang="ts">
    import { onMount } from 'svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'
    import { MAX_TEXT_PREVIEW_BYTES } from '$lib/utils/artifacts'
    import { createMarkdownParser } from '$lib/markdown/marked'
    import { srcdocDocument } from '../srcdoc'
    import ArtifactViewerFeedback from '../artifact-viewer-feedback.svelte'

    type Props = {
        artifact: ArtifactData
    }

    let { artifact }: Props = $props()

    let doc = $state<string | null>(null)
    let error = $state<string | null>(null)

    onMount(async () => {
        if (artifact.size_bytes > MAX_TEXT_PREVIEW_BYTES) {
            error = `File is ${Math.round(artifact.size_bytes / (1024 * 1024))} MB; the preview limit is 1 MB.`
            return
        }
        try {
            const response = await fetch(artifact.url)
            if (!response.ok) throw new Error(`Failed to load file (HTTP ${response.status})`)
            const text = await response.text()
            const html = await createMarkdownParser().parse(text)
            doc = srcdocDocument(html)
        } catch (err) {
            error = err instanceof Error ? err.message : 'Could not load the file.'
        }
    })
</script>

{#if doc}
    <!--
        Static document: no scripts/forms — the markdown renderer emits plain
        HTML, and any raw HTML in the source stays inert. Links use
        target=_blank, so popups are allowed, but never allow-same-origin.
    -->
    <iframe
        title={artifact.title}
        sandbox="allow-popups allow-popups-to-escape-sandbox"
        srcdoc={doc}
        class="h-full w-full border-0 bg-white"></iframe>
{:else}
    <ArtifactViewerFeedback
        state={error ? 'error' : 'loading'}
        message={error ?? 'Rendering markdown…'}
        {artifact} />
{/if}
