<script lang="ts">
    import { cn } from '../ui'
    let { class: className = '', ...rest }: { class?: string; [key: string]: unknown } = $props()
</script>
<input
    class={cn(
        'border-input bg-background ring-offset-background placeholder:text-muted-foreground focus-visible:ring-ring flex h-9 w-full rounded-md border px-3 py-1 text-sm shadow-sm transition-colors focus-visible:ring-2 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50',
        className,
    )}
    {...rest} />
