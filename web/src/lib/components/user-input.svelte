<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import * as Popover from '$lib/components/ui/popover'
    import {
        Send,
        Loader2,
        CircleStop,
        Search,
        MessageCircle,
        SendHorizontal,
    } from '@lucide/svelte'
    import { cn } from '$lib/utils'
    import type { Component } from 'svelte'
    import * as ToggleGroup from '$lib/components/ui/toggle-group'
    import * as Tooltip from '$lib/components/ui/tooltip'

    interface PopoverItem {
        label: string
        icon?: Component
        onClick: () => void
    }

    interface UserInputProps {
        value: string
        onSubmit: (inputMode: InputMode) => void | Promise<void>
        onInput: (value: string) => void
        modeSelectorEnabled: boolean
        placeholders?: Record<InputMode, string>
        isLoading?: boolean
        isStreaming?: boolean
        onStop?: () => void
        disabled?: boolean
        popoverItems?: PopoverItem[]
        showPopover?: boolean
        onPopoverChange?: (open: boolean) => void
        maxWidth?: string
        containerClass?: string
    }

    export type InputMode = 'search' | 'chat'
    const DEFAULT_PLACEHOLDERS: Record<InputMode, string> = {
        search: 'Search...',
        chat: 'Ask...',
    }

    let {
        value = $bindable(''),
        onSubmit,
        onInput,
        modeSelectorEnabled = true,
        placeholders = DEFAULT_PLACEHOLDERS,
        isLoading = false,
        isStreaming = false,
        onStop,
        disabled = false,
        popoverItems = [],
        showPopover = false,
        onPopoverChange,
        maxWidth = 'max-w-4xl',
        containerClass = '',
    }: UserInputProps = $props()

    let inputRef: HTMLDivElement
    let popoverContainer: HTMLDivElement | undefined = $state()
    let inputMode: InputMode = $state('chat')
    let placeholder = $derived(placeholders[inputMode])

    function handleKeyPress(event: KeyboardEvent) {
        if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault()
            handleSubmitClick()
        }
    }

    async function handleSubmitClick() {
        if (value.trim() && !disabled && !isLoading) {
            await onSubmit(inputMode)
        }
    }

    function handleStopClick() {
        if (onStop) {
            onStop()
        }
    }

    function handleInputChange() {
        if (inputRef) {
            onInput(inputRef.innerText)
        }
    }

    function handleFocus() {
        if (popoverItems.length > 0 && onPopoverChange) {
            onPopoverChange(true)
        }
    }

    function handleBlur() {
        if (onPopoverChange) {
            onPopoverChange(false)
        }
    }

    function handlePopoverItemClick(item: PopoverItem) {
        item.onClick()
        if (onPopoverChange) {
            onPopoverChange(false)
        }
    }
</script>

{#snippet modeSelector()}
    <ToggleGroup.Root
        variant="outline"
        size="sm"
        type="single"
        bind:value={inputMode}
        onclick={(e) => {
            e.stopPropagation()
        }}>
        <ToggleGroup.Item value="chat" aria-label="Toggle chat" class="cursor-pointer">
            <Tooltip.Provider delayDuration={300}>
                <Tooltip.Root>
                    <Tooltip.Trigger class="cursor-pointer">
                        <MessageCircle class="size-4" />
                    </Tooltip.Trigger>
                    <Tooltip.Content>
                        <div class="text-center">
                            <div class="font-semibold">Chat</div>
                            <div class="text-xs opacity-90">Have a conversation with AI</div>
                        </div>
                    </Tooltip.Content>
                </Tooltip.Root>
            </Tooltip.Provider>
        </ToggleGroup.Item>
        <ToggleGroup.Item value="search" aria-label="Toggle search" class="cursor-pointer">
            <Tooltip.Provider delayDuration={300}>
                <Tooltip.Root>
                    <Tooltip.Trigger class="cursor-pointer">
                        <Search class="size-4" />
                    </Tooltip.Trigger>
                    <Tooltip.Content>
                        <div class="text-center">
                            <div class="font-semibold">Search</div>
                            <div class="text-xs opacity-90">
                                Find information with AI-powered answers
                            </div>
                        </div>
                    </Tooltip.Content>
                </Tooltip.Root>
            </Tooltip.Provider>
        </ToggleGroup.Item>
    </ToggleGroup.Root>
{/snippet}

<div class={cn('w-full', maxWidth, containerClass)} bind:this={popoverContainer}>
    <div
        class={cn(
            'bg-card flex max-h-96 min-h-[1.5rem] w-full cursor-text flex-col gap-2 border border-gray-200 p-3 shadow-sm',
            showPopover && popoverItems.length > 0 ? 'rounded-t-xl' : 'rounded-xl',
        )}
        onclick={() => inputRef.focus()}
        onkeydown={handleKeyPress}
        role="button"
        tabindex="0">
        <div
            bind:this={inputRef}
            bind:innerText={value}
            oninput={handleInputChange}
            onfocus={handleFocus}
            onblur={handleBlur}
            class={cn(
                'before:text-muted-foreground relative min-h-12 cursor-text overflow-y-auto before:absolute before:inset-0 focus:outline-none',
                value.trim() ? "before:content-['']" : 'before:content-[attr(data-placeholder)]',
            )}
            contenteditable="true"
            role="textbox"
            aria-multiline="true"
            data-placeholder={placeholder}>
        </div>
        <div class="flex w-full items-end justify-between">
            {#if modeSelectorEnabled}
                {@render modeSelector()}
            {/if}
            <div class="flex w-full justify-end">
                {#if isStreaming}
                    <Button
                        size="icon"
                        class="cursor-pointer rounded-full"
                        onclick={handleStopClick}>
                        <CircleStop class="h-4 w-4" />
                    </Button>
                {:else if isLoading}
                    <Button size="icon" class="cursor-pointer" disabled>
                        <Loader2 class="h-4 w-4 animate-spin" />
                    </Button>
                {:else}
                    <Button
                        size="icon"
                        class="size-8 cursor-pointer"
                        onclick={handleSubmitClick}
                        disabled={!value.trim() || disabled}>
                        <SendHorizontal class="h-3 w-3" />
                    </Button>
                {/if}
            </div>
        </div>
    </div>

    {#if popoverItems.length > 0}
        <Popover.Root open={showPopover}>
            <Popover.Content
                class="w-full rounded-b-xl p-0"
                align="start"
                sideOffset={-1}
                alignOffset={-1}
                trapFocus={false}
                customAnchor={popoverContainer}
                onOpenAutoFocus={(e) => {
                    e.preventDefault()
                }}
                onCloseAutoFocus={(e) => {
                    e.preventDefault()
                }}
                onFocusOutside={(e) => e.preventDefault()}>
                <div class="max-w-2xl rounded-b-xl border bg-white">
                    <div class="py-2">
                        {#each popoverItems as item}
                            <button
                                class="hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground w-full px-4 py-2.5 text-left text-sm transition-colors focus:outline-none"
                                onclick={() => handlePopoverItemClick(item)}>
                                <div class="flex items-center gap-3">
                                    {#if item.icon}
                                        <svelte:component
                                            this={item.icon}
                                            class="text-muted-foreground h-4 w-4 shrink-0" />
                                    {/if}
                                    <span
                                        class="text-muted-foreground flex-1 truncate overflow-hidden text-sm"
                                        >{item.label}</span>
                                </div>
                            </button>
                        {/each}
                    </div>
                </div>
            </Popover.Content>
        </Popover.Root>
    {/if}
</div>
