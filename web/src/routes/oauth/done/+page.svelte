<script lang="ts">
    import { onMount } from 'svelte'

    let { data } = $props()

    onMount(() => {
        try {
            const ch = new BroadcastChannel('omni-user-auth')
            ch.postMessage({
                type: 'omni:user-auth-result',
                ok: data.ok,
                sourceId: data.sourceId,
                message: data.message,
            })
            ch.close()
        } catch {}
        setTimeout(() => {
            try {
                // window.close()
            } catch {}
        }, 50)
    })
</script>

<main class="flex min-h-screen items-center justify-center p-8 text-center">
    <p class="text-muted-foreground text-sm">
        {data.ok ? 'Connected.' : (data.message ?? 'Connection failed.')} You can close this window.
    </p>
</main>
