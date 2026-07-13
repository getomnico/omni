<script lang="ts" module>
	import { tv, type VariantProps } from "tailwind-variants";

	export const drawerVariants = tv({
		base: "bg-background data-[state=open]:animate-in data-[state=closed]:animate-out fixed z-50 flex flex-col shadow-lg transition ease-in-out data-[state=closed]:duration-300 data-[state=open]:duration-500",
		variants: {
			direction: {
				top: "data-[state=closed]:slide-out-to-top data-[state=open]:slide-in-from-top inset-x-0 top-0 max-h-[80dvh] border-b rounded-b-lg",
				bottom: "data-[state=closed]:slide-out-to-bottom data-[state=open]:slide-in-from-bottom inset-x-0 bottom-0 max-h-[80dvh] border-t rounded-t-lg",
				left: "data-[state=closed]:slide-out-to-left data-[state=open]:slide-in-from-left inset-y-0 left-0 h-full w-3/4 border-r sm:max-w-sm",
				right: "data-[state=closed]:slide-out-to-right data-[state=open]:slide-in-from-right inset-y-0 right-0 h-full w-3/4 border-l sm:max-w-sm",
			},
		},
		defaultVariants: {
			direction: "right",
		},
	});

	export type Direction = VariantProps<typeof drawerVariants>["direction"];
</script>

<script lang="ts">
	import { Drawer as DrawerPrimitive } from "vaul-svelte";
	import XIcon from "@lucide/svelte/icons/x";
	import type { Snippet } from "svelte";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		direction = "right",
		children,
		...restProps
	}: WithoutChildrenOrChild<DrawerPrimitive.ContentProps> & {
		direction?: Direction;
		children: Snippet;
	} = $props();
</script>

<DrawerPrimitive.Portal>
	<DrawerPrimitive.Overlay
		data-slot="drawer-overlay"
		class="data-[state=open]:animate-in data-[state=closed]:animate-out fixed inset-0 z-50 bg-black/80 data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0"
	/>
	<DrawerPrimitive.Content
		bind:ref
		data-slot="drawer-content"
		class={cn(drawerVariants({ direction }), className)}
		{...restProps}
	>
		{@render children?.()}
		{#if direction === "right" || direction === "left"}
			<DrawerPrimitive.Close
				class="ring-offset-background focus-visible:ring-ring rounded-xs focus-visible:outline-hidden absolute right-4 top-4 cursor-pointer opacity-70 transition-opacity hover:opacity-100 focus-visible:ring-2 focus-visible:ring-offset-2 disabled:pointer-events-none"
			>
				<XIcon class="size-4" />
				<span class="sr-only">Close</span>
			</DrawerPrimitive.Close>
		{/if}
	</DrawerPrimitive.Content>
</DrawerPrimitive.Portal>
