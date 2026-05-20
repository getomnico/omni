<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import { Loader2 } from '@lucide/svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import clickupLogo from '$lib/images/icons/clickup.svg'

    let { data }: PageProps = $props()

    let enabled = $state(data.source.isActive)

    let isSubmitting = $state(false)
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    let originalEnabled = data.source.isActive

    onMount(() => {
        beforeUnloadHandler = (e: BeforeUnloadEvent) => {
            if (hasUnsavedChanges && !skipUnsavedCheck) {
                e.preventDefault()
                e.returnValue = ''
            }
        }

        window.addEventListener('beforeunload', beforeUnloadHandler)

        return () => {
            if (beforeUnloadHandler) {
                window.removeEventListener('beforeunload', beforeUnloadHandler)
            }
        }
    })

    beforeNavigate(({ cancel }) => {
        if (hasUnsavedChanges && !skipUnsavedCheck) {
            const shouldLeave = confirm(
                'You have unsaved changes. Are you sure you want to leave this page?',
            )
            if (!shouldLeave) {
                cancel()
            }
        }
    })

    $effect(() => {
        hasUnsavedChanges = enabled !== originalEnabled
    })
</script>

<svelte:head>
    <title>Configure ClickUp - {data.source.name}</title>
</svelte:head>
<form
    method="POST"
    use:enhance={() => {
        isSubmitting = true
        return async ({ result, update }) => {
            if (result.type === 'redirect') {
                skipUnsavedCheck = true
                hasUnsavedChanges = false

                if (beforeUnloadHandler) {
                    window.removeEventListener('beforeunload', beforeUnloadHandler)
                    beforeUnloadHandler = null
                }
            }

            await update()
            isSubmitting = false
        }
    }}>
    <Card.Root class="relative">
        <Card.Header>
            <div class="flex items-start justify-between">
                <div>
                    <Card.Title class="flex items-center gap-2">
                        <img src={clickupLogo} alt="ClickUp" class="h-5 w-5" />
                        {data.source.name}
                    </Card.Title>
                    <Card.Description class="mt-1">
                        Index tasks and docs from ClickUp workspaces
                    </Card.Description>
                </div>
                <div class="flex items-center gap-2">
                    <Label for="enabled" class="text-sm">Enabled</Label>
                    <Switch
                        id="enabled"
                        bind:checked={enabled}
                        name="enabled"
                        class="cursor-pointer" />
                </div>
            </div>
        </Card.Header>

        <Card.Content>
            <p class="text-muted-foreground text-sm">
                All tasks and docs accessible to the connected account will be indexed.
            </p>
        </Card.Content>
        <Card.Footer class="flex justify-end">
            <Button
                type="submit"
                disabled={isSubmitting || !hasUnsavedChanges}
                class="cursor-pointer">
                {#if isSubmitting}
                    <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                {/if}
                Save Configuration
            </Button>
        </Card.Footer>
    </Card.Root>
</form>
