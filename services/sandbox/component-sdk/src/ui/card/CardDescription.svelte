<script lang="ts">
    import { cn } from '../../ui.ts'
    let { class: className = '', children, ...rest }: { class?: string; children: import('svelte').Snippet; [key: string]: unknown } = $props()
</script>
<p class={cn('text-muted-foreground text-sm', className)} {...rest}>{@render children()}</p>
