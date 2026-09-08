<script lang="ts">
    import type { ArtifactData } from '$lib/utils/artifacts'

    type Props = {
        artifact: ArtifactData
    }

    let { artifact }: Props = $props()
</script>

<!-- Native browser PDF viewer; the artifact URL is same-origin proxied. -->
<iframe title={artifact.title} src={artifact.url} class="bg-muted h-full w-full border-0"></iframe>
