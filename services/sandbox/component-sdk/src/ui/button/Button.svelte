<script lang="ts">
    import { cn } from '../../ui.ts'

    type Variant = 'default' | 'secondary' | 'outline' | 'ghost' | 'destructive'
    type Size = 'default' | 'sm' | 'lg' | 'icon'

    let {
        variant = 'default',
        size = 'default',
        class: className = '',
        children,
        ...rest
    }: {
        variant?: Variant
        size?: Size
        class?: string
        children: import('svelte').Snippet
        [key: string]: unknown
    } = $props()

    const variants: Record<Variant, string> = {
        default: 'bg-primary text-primary-foreground hover:bg-primary/90',
        secondary: 'bg-secondary text-secondary-foreground hover:bg-secondary/80',
        outline: 'border border-input bg-background hover:bg-accent hover:text-accent-foreground',
        ghost: 'hover:bg-accent hover:text-accent-foreground',
        destructive: 'bg-destructive text-white hover:bg-destructive/90',
    }
    const sizes: Record<Size, string> = {
        default: 'h-9 px-4 py-2',
        sm: 'h-8 rounded-md px-3 text-xs',
        lg: 'h-10 rounded-md px-8',
        icon: 'h-9 w-9',
    }
</script>

<button
    class={cn(
        'inline-flex cursor-pointer items-center justify-center gap-2 rounded-md text-sm font-medium whitespace-nowrap transition-colors focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none disabled:pointer-events-none disabled:opacity-50',
        variants[variant],
        sizes[size],
        className,
    )}
    {...rest}>
    {@render children()}
</button>
