<script lang="ts">
    import { cn } from '../ui'
    let { class: className = '', children, ...rest }: { class?: string; children: import('svelte').Snippet; [key: string]: unknown } = $props()
</script>
<div class={cn('flex flex-col space-y-1.5 p-6', className)} {...rest}>{@render children()}</div>
