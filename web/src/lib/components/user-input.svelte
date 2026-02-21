<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import * as Popover from '$lib/components/ui/popover'
    import * as Select from '$lib/components/ui/select'
    import {
        Send,
        Loader2,
        CircleStop,
        Search,
        MessageCircle,
        SendHorizontal,
        FileText,
    } from '@lucide/svelte'
    import { cn } from '$lib/utils'
    import type { Component } from 'svelte'
    import * as ToggleGroup from '$lib/components/ui/toggle-group'
    import * as Tooltip from '$lib/components/ui/tooltip'
    import type { TypeaheadResult } from '$lib/types/search'

    interface PopoverItem {
        label: string
        icon?: Component
        onClick: () => void
    }

    export interface ModelOption {
        id: string
        displayName: string
        providerType: string
        isDefault: boolean
    }

    interface UserInputProps {
        value: string
        inputMode: InputMode
        onSubmit: () => void | Promise<void>
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
        models?: ModelOption[]
        selectedModelId?: string | null
        onModelChange?: (modelId: string) => void
    }

    export type InputMode = 'search' | 'chat'
    const DEFAULT_PLACEHOLDERS: Record<InputMode, string> = {
        search: 'Search...',
        chat: 'Ask...',
    }

    let {
        value = $bindable(''),
        inputMode = $bindable(),
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
        models = [],
        selectedModelId = null,
        onModelChange,
    }: UserInputProps = $props()

    let showModelSelector = $derived(models.length >= 2 && inputMode === 'chat')

    const providerDisplayNames: Record<string, string> = {
        anthropic: 'Anthropic',
        openai: 'OpenAI',
        vllm: 'vLLM',
        bedrock: 'Bedrock',
    }

    function formatProviderName(provider: string): string {
        return (
            providerDisplayNames[provider] ?? provider.charAt(0).toUpperCase() + provider.slice(1)
        )
    }

    let groupedModels = $derived(
        Object.entries(
            models.reduce<Record<string, ModelOption[]>>((acc, m) => {
                if (!acc[m.providerType]) {
                    acc[m.providerType] = []
                }
                acc[m.providerType].push(m)
                return acc
            }, {}),
        ),
    )

    let inputRef: HTMLDivElement
    let popoverContainer: HTMLDivElement | undefined = $state()
    let placeholder = $derived(placeholders[inputMode])

    // @-mention state
    let mentionActive = $state(false)
    let mentionQuery = $state('')
    let mentionResults: TypeaheadResult[] = $state([])
    let mentionHighlightIndex = $state(0)
    let debounceTimer: ReturnType<typeof setTimeout> | undefined

    // Mention anchor: the text node and offset where `@` was found
    let mentionAnchorNode: Text | null = null
    let mentionAnchorOffset = 0

    let effectivePopoverItems: PopoverItem[] = $derived(
        mentionActive && mentionResults.length > 0
            ? mentionResults.map((result) => ({
                  label: result.title,
                  icon: FileText,
                  onClick: () => insertMentionChip(result),
              }))
            : popoverItems,
    )

    let effectiveShowPopover = $derived(
        mentionActive && mentionResults.length > 0 ? true : showPopover,
    )

    $effect(() => {
        if (inputRef && value !== inputRef.textContent) {
            inputRef.textContent = value
        }
    })

    function closeMention() {
        mentionActive = false
        mentionQuery = ''
        mentionResults = []
        mentionHighlightIndex = 0
        mentionAnchorNode = null
        mentionAnchorOffset = 0
        if (debounceTimer) {
            clearTimeout(debounceTimer)
            debounceTimer = undefined
        }
    }

    function detectMention() {
        const sel = window.getSelection()
        if (!sel || sel.rangeCount === 0) {
            closeMention()
            return
        }

        const range = sel.getRangeAt(0)
        const node = range.startContainer
        if (node.nodeType !== Node.TEXT_NODE) {
            closeMention()
            return
        }

        const text = node.textContent || ''
        const cursorPos = range.startOffset

        // Scan backwards from cursor for `@`
        let atIndex = -1
        for (let i = cursorPos - 1; i >= 0; i--) {
            const ch = text[i]
            if (ch === '@') {
                // `@` must be at start of text or preceded by whitespace
                if (i === 0 || /\s/.test(text[i - 1])) {
                    atIndex = i
                }
                break
            }
            // Stop scanning if we hit whitespace before finding `@`
            if (/\s/.test(ch)) break
        }

        if (atIndex === -1) {
            closeMention()
            return
        }

        const query = text.slice(atIndex + 1, cursorPos)
        mentionAnchorNode = node as Text
        mentionAnchorOffset = atIndex

        if (query.length < 2) {
            mentionActive = true
            mentionQuery = query
            mentionResults = []
            return
        }

        mentionActive = true
        mentionQuery = query
        mentionHighlightIndex = 0

        if (debounceTimer) clearTimeout(debounceTimer)
        debounceTimer = setTimeout(() => {
            fetchTypeahead(query)
        }, 150)
    }

    async function fetchTypeahead(query: string) {
        try {
            const res = await fetch(`/api/typeahead?q=${encodeURIComponent(query)}&limit=5`)
            if (!res.ok) {
                mentionResults = []
                return
            }
            const data = await res.json()
            if (mentionActive && mentionQuery === query) {
                mentionResults = data.results || []
                mentionHighlightIndex = 0
            }
        } catch {
            mentionResults = []
        }
    }

    function insertMentionChip(result: TypeaheadResult) {
        if (!mentionAnchorNode || !inputRef) return

        const sel = window.getSelection()
        if (!sel || sel.rangeCount === 0) return

        const cursorOffset = sel.getRangeAt(0).startOffset

        // Create a range from `@` to current cursor position
        const range = document.createRange()
        range.setStart(mentionAnchorNode, mentionAnchorOffset)
        range.setEnd(mentionAnchorNode, cursorOffset)
        range.deleteContents()

        // Create the chip element
        const chip = document.createElement('span')
        chip.contentEditable = 'false'
        chip.dataset.documentId = result.document_id
        chip.className =
            'inline-flex items-center rounded-full bg-blue-100 text-blue-800 px-2 text-sm select-none'
        chip.textContent = result.title

        // Insert chip + trailing space
        range.insertNode(chip)

        const space = document.createTextNode('\u00A0')
        chip.after(space)

        // Move cursor after the space
        const newRange = document.createRange()
        newRange.setStartAfter(space)
        newRange.collapse(true)
        sel.removeAllRanges()
        sel.addRange(newRange)

        closeMention()
        onInput(inputRef.innerText)
    }

    function handleKeyPress(event: KeyboardEvent) {
        if (mentionActive && mentionResults.length > 0) {
            if (event.key === 'ArrowDown') {
                event.preventDefault()
                mentionHighlightIndex = (mentionHighlightIndex + 1) % mentionResults.length
                return
            }
            if (event.key === 'ArrowUp') {
                event.preventDefault()
                mentionHighlightIndex =
                    (mentionHighlightIndex - 1 + mentionResults.length) % mentionResults.length
                return
            }
            if (event.key === 'Enter') {
                event.preventDefault()
                insertMentionChip(mentionResults[mentionHighlightIndex])
                return
            }
            if (event.key === 'Escape') {
                event.preventDefault()
                closeMention()
                return
            }
        }

        if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault()
            handleSubmitClick()
        }
    }

    async function handleSubmitClick() {
        if (value.trim() && !disabled && !isLoading) {
            await onSubmit()
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
            detectMention()
        }
    }

    function handleFocus() {
        if (popoverItems.length > 0 && onPopoverChange) {
            onPopoverChange(true)
        }
    }

    function handleBlur() {
        // Delay to allow popover item clicks to register
        setTimeout(() => {
            if (!mentionActive && onPopoverChange) {
                onPopoverChange(false)
            }
        }, 150)
    }

    function handlePopoverItemClick(item: PopoverItem) {
        item.onClick()
        if (!mentionActive && onPopoverChange) {
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
            effectiveShowPopover && effectivePopoverItems.length > 0
                ? 'rounded-t-xl'
                : 'rounded-xl',
        )}
        onclick={() => inputRef.focus()}
        onkeydown={handleKeyPress}
        role="button"
        tabindex="0">
        <div
            bind:this={inputRef}
            oninput={handleInputChange}
            onfocus={handleFocus}
            onblur={handleBlur}
            class={cn(
                'before:text-muted-foreground relative min-h-12 cursor-text overflow-y-auto before:pointer-events-none before:absolute before:inset-0 focus:outline-none',
                value.trim() ? "before:content-['']" : 'before:content-[attr(data-placeholder)]',
            )}
            contenteditable="true"
            role="textbox"
            aria-multiline="true"
            data-placeholder={placeholder}>
        </div>
        <div class="flex w-full items-end justify-between">
            <div class="flex items-center gap-2">
                {#if modeSelectorEnabled}
                    {@render modeSelector()}
                {/if}
            </div>
            <div class="flex w-full justify-end gap-2">
                {#if showModelSelector}
                    <Select.Root
                        type="single"
                        value={selectedModelId ?? undefined}
                        onValueChange={(v) => {
                            if (v && onModelChange) onModelChange(v)
                        }}>
                        <Select.Trigger
                            size="sm"
                            class="hover:bg-muted text-muted-foreground h-8 max-w-[180px] cursor-pointer border-none text-sm shadow-none"
                            onclick={(e) => e.stopPropagation()}>
                            {models.find((m) => m.id === selectedModelId)?.displayName ??
                                'Select model'}
                        </Select.Trigger>
                        <Select.Content align="end">
                            {#each groupedModels as [provider, providerModels]}
                                <Select.Group>
                                    <Select.GroupHeading>
                                        {formatProviderName(provider)}
                                    </Select.GroupHeading>
                                    {#each providerModels as model}
                                        <Select.Item value={model.id}>
                                            {model.displayName}
                                        </Select.Item>
                                    {/each}
                                </Select.Group>
                            {/each}
                        </Select.Content>
                    </Select.Root>
                {/if}
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

    {#if effectivePopoverItems.length > 0}
        <Popover.Root open={effectiveShowPopover}>
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
                        {#each effectivePopoverItems as item, i}
                            <button
                                class={cn(
                                    'hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground w-full px-4 py-2.5 text-left text-sm transition-colors focus:outline-none',
                                    mentionActive &&
                                        i === mentionHighlightIndex &&
                                        'bg-accent text-accent-foreground',
                                )}
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
