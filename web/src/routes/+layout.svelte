<script lang="ts">
    import '../app.css'
    import { Toaster } from '$lib/components/ui/sonner'
    import { registerServiceWorker } from '$lib/pwa/service-worker-client'
    import { onMount } from 'svelte'

    let { children } = $props()

    onMount(() => {
        void registerServiceWorker()
    })
</script>

<Toaster position="top-right" />
{@render children()}
