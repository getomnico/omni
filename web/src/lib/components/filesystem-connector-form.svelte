<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Collapsible from '$lib/components/ui/collapsible'
    import { X, ChevronDown, ChevronRight } from '@lucide/svelte'

    interface Props {
        name?: string
        basePath?: string
        fileExtensions?: string[]
        excludePatterns?: string[]
        maxFileSizeMb?: number
        disabled?: boolean
    }

    let {
        name = $bindable(''),
        basePath = $bindable(''),
        fileExtensions = $bindable([]),
        excludePatterns = $bindable([]),
        maxFileSizeMb = $bindable(10),
        disabled = false,
    }: Props = $props()

    let advancedOpen = $state(false)
    let extensionInput = $state('')
    let patternInput = $state('')

    function addExtension() {
        const ext = extensionInput.trim().replace(/^\./, '')
        if (ext && !fileExtensions.includes(ext)) {
            fileExtensions = [...fileExtensions, ext]
            extensionInput = ''
        }
    }

    function removeExtension(ext: string) {
        fileExtensions = fileExtensions.filter((e) => e !== ext)
    }

    function addPattern() {
        const pattern = patternInput.trim()
        if (pattern && !excludePatterns.includes(pattern)) {
            excludePatterns = [...excludePatterns, pattern]
            patternInput = ''
        }
    }

    function removePattern(pattern: string) {
        excludePatterns = excludePatterns.filter((p) => p !== pattern)
    }
</script>

<div class="space-y-6">
    <!-- Name -->
    <div class="space-y-2">
        <Label for="name">Name *</Label>
        <Input
            id="name"
            name="name"
            bind:value={name}
            placeholder="My Local Files"
            {disabled}
            required />
        <p class="text-muted-foreground text-sm">A friendly name to identify this source</p>
    </div>

    <!-- Base Path -->
    <div class="space-y-2">
        <Label for="basePath">Base Path *</Label>
        <Input
            id="basePath"
            name="basePath"
            bind:value={basePath}
            placeholder="/data/documents"
            {disabled}
            required />
        <p class="text-muted-foreground text-sm">
            Absolute path to the directory to index. Must be accessible from the server.
        </p>
    </div>

    <!-- Advanced Settings -->
    <Collapsible.Root bind:open={advancedOpen}>
        <Collapsible.Trigger
            class="flex w-full items-center gap-2 text-left text-sm font-medium hover:underline disabled:opacity-50"
            {disabled}>
            {#if advancedOpen}
                <ChevronDown class="h-4 w-4" />
            {:else}
                <ChevronRight class="h-4 w-4" />
            {/if}
            Advanced Settings
        </Collapsible.Trigger>

        <Collapsible.Content>
            <div class="space-y-6 pt-4">
                <!-- File Extensions -->
                <div class="space-y-3">
                    <Label>File Extensions</Label>
                    <p class="text-muted-foreground text-sm">
                        Only index files with these extensions. Leave empty to index all text files.
                    </p>

                    <div class="flex gap-2">
                        <Input
                            bind:value={extensionInput}
                            placeholder="e.g., txt, md, json"
                            {disabled}
                            class="flex-1"
                            onkeydown={(e) => {
                                if (e.key === 'Enter') {
                                    e.preventDefault()
                                    addExtension()
                                }
                            }} />
                        <Button
                            type="button"
                            variant="secondary"
                            onclick={addExtension}
                            disabled={disabled || !extensionInput.trim()}>
                            Add
                        </Button>
                    </div>

                    {#if fileExtensions.length > 0}
                        <div class="flex flex-wrap gap-2">
                            {#each fileExtensions as ext}
                                <div
                                    class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors">
                                    <span>.{ext}</span>
                                    <button
                                        type="button"
                                        onclick={() => removeExtension(ext)}
                                        class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                                        aria-label="Remove {ext}">
                                        <X class="h-3 w-3" />
                                    </button>
                                </div>
                            {/each}
                        </div>
                    {/if}
                </div>

                <!-- Exclude Patterns -->
                <div class="space-y-3">
                    <Label>Exclude Patterns</Label>
                    <p class="text-muted-foreground text-sm">
                        Glob patterns for files/directories to exclude.
                    </p>

                    <div class="flex gap-2">
                        <Input
                            bind:value={patternInput}
                            placeholder="e.g., node_modules, *.log, .git"
                            {disabled}
                            class="flex-1"
                            onkeydown={(e) => {
                                if (e.key === 'Enter') {
                                    e.preventDefault()
                                    addPattern()
                                }
                            }} />
                        <Button
                            type="button"
                            variant="secondary"
                            onclick={addPattern}
                            disabled={disabled || !patternInput.trim()}>
                            Add
                        </Button>
                    </div>

                    {#if excludePatterns.length > 0}
                        <div class="flex flex-wrap gap-2">
                            {#each excludePatterns as pattern}
                                <div
                                    class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors">
                                    <span>{pattern}</span>
                                    <button
                                        type="button"
                                        onclick={() => removePattern(pattern)}
                                        class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                                        aria-label="Remove {pattern}">
                                        <X class="h-3 w-3" />
                                    </button>
                                </div>
                            {/each}
                        </div>
                    {/if}
                </div>

                <!-- Max File Size and Scan Interval -->
                <div class="grid grid-cols-2 gap-4">
                    <div class="space-y-2">
                        <Label for="maxFileSizeMb">Max File Size (MB)</Label>
                        <Input
                            id="maxFileSizeMb"
                            name="maxFileSizeMb"
                            type="number"
                            min="1"
                            bind:value={maxFileSizeMb}
                            {disabled}
                            placeholder="10" />
                        <p class="text-muted-foreground text-sm">Skip files larger than this</p>
                    </div>
                </div>
            </div>
        </Collapsible.Content>
    </Collapsible.Root>

    <!-- Hidden inputs for form submission -->
    {#each fileExtensions as ext}
        <input type="hidden" name="fileExtensions" value={ext} />
    {/each}
    {#each excludePatterns as pattern}
        <input type="hidden" name="excludePatterns" value={pattern} />
    {/each}
</div>
