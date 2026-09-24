<script lang="ts">
    import { cn } from '../../ui.ts'

    type Props = {
        class?: string
        children: import('svelte').Snippet
        [key: string]: unknown
    }
    let { class: className = '', children, ...rest }: Props = $props()
</script>

<div class={cn('bg-card text-card-foreground rounded-xl border shadow-sm', className)} {...rest}>
    {@render children()}
</div>
