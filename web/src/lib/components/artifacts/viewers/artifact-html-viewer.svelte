<script lang="ts">
    import type { ArtifactData } from '$lib/utils/artifacts'

    type Props = {
        artifact: ArtifactData
    }

    let { artifact }: Props = $props()
</script>

<!--
    HTML artifacts run sandboxed. `allow-scripts` lets generated pages work, but
    `allow-same-origin` is deliberately absent: the page lives in a unique origin
    and can never read app cookies or storage.
-->
<iframe
    title={artifact.title}
    src={artifact.url}
    sandbox="allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads"
    class="h-full w-full border-0 bg-white"></iframe>
