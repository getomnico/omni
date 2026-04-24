<script lang="ts">
    import { Input } from '$lib/components/ui/input'
    import { Textarea } from '$lib/components/ui/textarea'
    import { Button } from '$lib/components/ui/button'

    interface Props {
        id: string
        name?: string
        value: string
        hasStoredValue: boolean
        multiline?: boolean
        disabled?: boolean
        placeholder?: string
        rows?: number
        inputClass?: string
    }

    let {
        id,
        name,
        value = $bindable(''),
        hasStoredValue,
        multiline = false,
        disabled = false,
        placeholder = '',
        rows = 10,
        inputClass = '',
    }: Props = $props()

    let replacing = $state(!hasStoredValue)

    const maskedPlaceholder = '•••••••• (leave empty to keep current)'

    function startReplace() {
        replacing = true
        value = ''
    }

    function cancelReplace() {
        replacing = false
        value = ''
    }
</script>

{#if hasStoredValue && !replacing}
    <div class="flex items-center gap-2">
        <Input {id} type="text" value={maskedPlaceholder} disabled class={inputClass} readonly />
        <Button
            type="button"
            variant="outline"
            onclick={startReplace}
            {disabled}
            class="shrink-0 cursor-pointer">
            Replace
        </Button>
    </div>
{:else if multiline}
    <div class="space-y-2">
        <Textarea
            {id}
            {name}
            bind:value
            {disabled}
            {placeholder}
            {rows}
            class={inputClass ||
                'max-h-64 overflow-y-auto font-mono text-sm break-all whitespace-pre-wrap'} />
        {#if hasStoredValue}
            <Button
                type="button"
                variant="ghost"
                size="sm"
                onclick={cancelReplace}
                {disabled}
                class="cursor-pointer">
                Cancel replacement
            </Button>
        {/if}
    </div>
{:else}
    <div class="flex items-center gap-2">
        <Input {id} {name} type="password" bind:value {disabled} {placeholder} class={inputClass} />
        {#if hasStoredValue}
            <Button
                type="button"
                variant="ghost"
                size="sm"
                onclick={cancelReplace}
                {disabled}
                class="shrink-0 cursor-pointer">
                Cancel
            </Button>
        {/if}
    </div>
{/if}
