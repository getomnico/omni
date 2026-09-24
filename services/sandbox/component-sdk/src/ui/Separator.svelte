<script lang="ts">
    import { cn } from './ui'
    let { orientation = 'horizontal', class: className = '', ...rest }: { orientation?: 'horizontal' | 'vertical'; class?: string; [key: string]: unknown } = $props()
</script>
<div role="separator" aria-orientation={orientation} class={cn('bg-border shrink-0', orientation === 'horizontal' ? 'h-px w-full' : 'h-full w-px', className)} {...rest}></div>
