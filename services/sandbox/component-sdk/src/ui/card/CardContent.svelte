<script lang="ts">
    import { cn } from '../../ui.ts'
    let { class: className = '', children, ...rest }: { class?: string; children: import('svelte').Snippet; [key: string]: unknown } = $props()
</script>
<div class={cn('p-6 pt-0', className)} {...rest}>{@render children()}</div>
