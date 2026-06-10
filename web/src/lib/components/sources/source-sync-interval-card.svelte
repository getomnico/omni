<script lang="ts">
    import { invalidateAll } from '$app/navigation'
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Select from '$lib/components/ui/select'
    import {
        deriveCustomSyncInterval,
        formatSyncInterval,
        isValidSyncIntervalSeconds,
        MAX_SYNC_INTERVAL_SECONDS,
        MIN_SYNC_INTERVAL_SECONDS,
        SYNC_INTERVAL_PRESETS,
        SYNC_INTERVAL_UNIT_SECONDS,
        type SyncIntervalUnit,
    } from '$lib/utils/sync-interval'
    import { Loader2 } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'

    interface Props {
        sourceId: string
        syncIntervalSeconds: number | null
    }

    let { sourceId, syncIntervalSeconds }: Props = $props()

    let selectedInterval = $state('3600')
    let customValue = $state('1')
    let customUnit = $state<SyncIntervalUnit>('hours')
    let isSaving = $state(false)
    let errorMessage = $state('')
    let lastLoadedSeconds = $state<number | null | undefined>(undefined)

    const unitOptions: { value: SyncIntervalUnit; label: string }[] = [
        { value: 'minutes', label: 'Minutes' },
        { value: 'hours', label: 'Hours' },
        { value: 'days', label: 'Days' },
    ]

    const selectedSeconds = $derived(getSelectedSeconds())
    const selectedIntervalLabel = $derived(
        selectedInterval === 'custom'
            ? selectedSeconds
                ? `Custom: ${formatSyncInterval(selectedSeconds)}`
                : 'Custom'
            : SYNC_INTERVAL_PRESETS.find((option) => option.value === selectedInterval)?.label ||
                  'Select interval',
    )
    const customUnitLabel = $derived(
        unitOptions.find((option) => option.value === customUnit)?.label || 'Unit',
    )
    const validationMessage = $derived(getValidationMessage())
    const hasChanges = $derived(
        selectedSeconds !== null &&
            (syncIntervalSeconds === null || selectedSeconds !== syncIntervalSeconds),
    )

    function applySeconds(seconds: number | null | undefined) {
        const preset = SYNC_INTERVAL_PRESETS.find((option) => option.seconds === seconds)

        if (preset) {
            selectedInterval = preset.value
        } else if (seconds && seconds > 0) {
            const custom = deriveCustomSyncInterval(seconds)
            selectedInterval = 'custom'
            customValue = String(custom.value)
            customUnit = custom.unit
        } else {
            selectedInterval = '3600'
            customValue = '1'
            customUnit = 'hours'
        }

        errorMessage = ''
        lastLoadedSeconds = seconds
    }

    $effect(() => {
        if (!isSaving && syncIntervalSeconds !== lastLoadedSeconds) {
            applySeconds(syncIntervalSeconds)
        }
    })

    function getSelectedSeconds() {
        if (selectedInterval !== 'custom') {
            const preset = SYNC_INTERVAL_PRESETS.find((option) => option.value === selectedInterval)
            return preset?.seconds ?? null
        }

        const trimmedValue = customValue.trim()
        if (!/^\d+$/.test(trimmedValue)) {
            return null
        }

        const value = Number(trimmedValue)
        if (!Number.isSafeInteger(value) || value <= 0) {
            return null
        }

        return value * SYNC_INTERVAL_UNIT_SECONDS[customUnit]
    }

    function getValidationMessage() {
        const seconds = getSelectedSeconds()

        if (selectedInterval === 'custom' && !/^\d+$/.test(customValue.trim())) {
            return 'Custom interval must be a positive whole number.'
        }

        if (seconds === null || !isValidSyncIntervalSeconds(seconds)) {
            return `Sync interval must be between ${formatSyncInterval(MIN_SYNC_INTERVAL_SECONDS).toLowerCase()} and ${formatSyncInterval(MAX_SYNC_INTERVAL_SECONDS).toLowerCase()}.`
        }

        return ''
    }

    async function handleSave() {
        const seconds = getSelectedSeconds()
        const message = getValidationMessage()

        if (message || seconds === null) {
            errorMessage = message || 'Choose a valid sync interval.'
            toast.error(errorMessage)
            return
        }

        isSaving = true
        errorMessage = ''

        try {
            const response = await fetch(`/api/sources/${sourceId}`, {
                method: 'PATCH',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ syncIntervalSeconds: seconds }),
            })

            if (!response.ok) {
                const result = await response.json().catch(() => null)
                throw new Error(
                    result?.message || result?.error || 'Failed to update sync interval',
                )
            }

            const result = (await response.json()) as { syncIntervalSeconds: number }
            applySeconds(result.syncIntervalSeconds)
            toast.success('Sync interval updated')
            await invalidateAll()
        } catch (err) {
            errorMessage = err instanceof Error ? err.message : 'Failed to update sync interval'
            toast.error(errorMessage)
        } finally {
            isSaving = false
        }
    }
</script>

<Card.Root>
    <Card.Header>
        <Card.Title>Sync Interval</Card.Title>
        <Card.Description>Choose how often Omni should sync this source</Card.Description>
    </Card.Header>
    <form
        onsubmit={(event) => {
            event.preventDefault()
            handleSave()
        }}>
        <Card.Content class="space-y-4">
            <div class="space-y-2">
                <Label for="sync-interval-preset">Interval</Label>
                <Select.Root
                    type="single"
                    disabled={isSaving}
                    value={selectedInterval}
                    onValueChange={(value) => {
                        if (!value) return
                        selectedInterval = value
                        errorMessage = ''
                    }}>
                    <Select.Trigger id="sync-interval-preset" class="w-full cursor-pointer sm:w-72">
                        {selectedIntervalLabel}
                    </Select.Trigger>
                    <Select.Content>
                        {#each SYNC_INTERVAL_PRESETS as preset (preset.value)}
                            <Select.Item value={preset.value} class="cursor-pointer">
                                {preset.label}
                            </Select.Item>
                        {/each}
                        <Select.Item value="custom" class="cursor-pointer">Custom</Select.Item>
                    </Select.Content>
                </Select.Root>
            </div>

            {#if selectedInterval === 'custom'}
                <div class="flex flex-col gap-3 sm:flex-row">
                    <div class="space-y-2">
                        <Label for="custom-sync-interval-value">Custom interval</Label>
                        <Input
                            id="custom-sync-interval-value"
                            type="text"
                            inputmode="numeric"
                            pattern="[0-9]*"
                            disabled={isSaving}
                            bind:value={customValue}
                            class="sm:w-32"
                            aria-invalid={Boolean(validationMessage)} />
                    </div>
                    <div class="space-y-2">
                        <Label for="custom-sync-interval-unit">Unit</Label>
                        <Select.Root
                            type="single"
                            disabled={isSaving}
                            value={customUnit}
                            onValueChange={(value) => {
                                if (!value) return
                                customUnit = value as SyncIntervalUnit
                                errorMessage = ''
                            }}>
                            <Select.Trigger
                                id="custom-sync-interval-unit"
                                class="w-full cursor-pointer">
                                {customUnitLabel}
                            </Select.Trigger>
                            <Select.Content>
                                {#each unitOptions as option (option.value)}
                                    <Select.Item value={option.value} class="cursor-pointer">
                                        {option.label}
                                    </Select.Item>
                                {/each}
                            </Select.Content>
                        </Select.Root>
                    </div>
                </div>
            {/if}

            {#if validationMessage || errorMessage}
                <p class="text-destructive text-sm">{errorMessage || validationMessage}</p>
            {/if}
        </Card.Content>
        <Card.Footer class="mt-4 flex justify-end border-t">
            <Button type="submit" disabled={isSaving || Boolean(validationMessage) || !hasChanges}>
                {#if isSaving}
                    <Loader2 class="h-4 w-4 animate-spin" />
                    Saving...
                {:else}
                    Save interval
                {/if}
            </Button>
        </Card.Footer>
    </form>
</Card.Root>
