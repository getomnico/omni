<script lang="ts">
    import { onMount } from 'svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'
    import { MAX_DOCX_PREVIEW_BYTES } from '$lib/utils/artifacts'
    import { srcdocDocument } from '../srcdoc'
    import ArtifactViewerFeedback from '../artifact-viewer-feedback.svelte'

    type Props = {
        artifact: ArtifactData
    }

    let { artifact }: Props = $props()

    let doc = $state<string | null>(null)
    let error = $state<string | null>(null)

    onMount(async () => {
        if (artifact.size_bytes > MAX_DOCX_PREVIEW_BYTES) {
            error = `File is ${Math.round(artifact.size_bytes / (1024 * 1024))} MB; the preview limit is 25 MB.`
            return
        }
        try {
            const response = await fetch(artifact.url)
            if (!response.ok) {
                throw new Error(`Failed to load file (HTTP ${response.status})`)
            }
            const arrayBuffer = await response.arrayBuffer()
            const mammoth = await import('mammoth/mammoth.browser.min.js')
            const convertToHtml = mammoth.convertToHtml ?? mammoth.default.convertToHtml
            const result = await convertToHtml({ arrayBuffer })
            doc = srcdocDocument(result.value)
        } catch (err) {
            error = err instanceof Error ? err.message : 'Could not convert the document.'
        }
    })
</script>

{#if doc}
    <iframe title={artifact.title} sandbox="" srcdoc={doc} class="h-full w-full border-0 bg-white"
    ></iframe>
{:else}
    <ArtifactViewerFeedback
        state={error ? 'error' : 'loading'}
        message={error ?? 'Converting Word document…'}
        {artifact} />
{/if}
