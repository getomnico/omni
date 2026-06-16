<script lang="ts">
    import '../app.css'
    import DeploymentVersion from '$lib/components/deployment-version.svelte'
    import { Toaster } from '$lib/components/ui/sonner'

    let { children } = $props()
</script>

<DeploymentVersion />
<Toaster position="top-right" />
{@render children()}
