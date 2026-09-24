<script lang="ts">
    import { cn } from '../../ui.ts'
    let { class: className = '', children, ...rest }: { class?: string; children: import('svelte').Snippet; [key: string]: unknown } = $props()
</script>
<span class={cn('bg-primary text-primary-foreground inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors', className)} {...rest}>{@render children()}</span>
