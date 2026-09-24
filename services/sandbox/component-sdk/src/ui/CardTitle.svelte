<script lang="ts">
    import { cn } from '../ui'
    let { class: className = '', children, ...rest }: { class?: string; children: import('svelte').Snippet; [key: string]: unknown } = $props()
</script>
<h3 class={cn('text-lg leading-none font-semibold tracking-tight', className)} {...rest}>{@render children()}</h3>
