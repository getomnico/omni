<script lang="ts">
    import { DropdownMenu as DropdownMenuPrimitive } from 'bits-ui'
    import type { Snippet } from 'svelte'
    import { cn, type WithoutChildrenOrChild } from '$lib/utils.js'

    let {
        ref = $bindable(null),
        class: className,
        children,
        ...restProps
    }: WithoutChildrenOrChild<DropdownMenuPrimitive.ItemProps> & {
        children: Snippet
    } = $props()
</script>

<DropdownMenuPrimitive.Item
    bind:ref
    data-slot="dropdown-menu-item"
    class={cn(
        'focus:bg-accent focus:text-accent-foreground relative flex cursor-default select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden transition-colors data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*="size-"])]:size-4',
        className,
    )}
    {...restProps}
>
    {@render children?.()}
</DropdownMenuPrimitive.Item>
