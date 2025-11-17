<script lang="ts">
    import { goto } from '$app/navigation'
    import { onMount } from 'svelte'

    onMount(() => {
        goto('/admin/settings/integrations', { replaceState: true })
    })
</script>
