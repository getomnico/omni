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
    import notionLogo from '$lib/images/icons/notion.svg'

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
    <title>Configure Notion - {data.source.name}</title>
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
                        <img src={notionLogo} alt="Notion" class="h-5 w-5" />
                        {data.source.name}
                    </Card.Title>
                    <Card.Description class="mt-1">
                        All pages and databases accessible to the connected integration will be
                        indexed.
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

        <Card.Content class="space-y-3">
            <p class="text-muted-foreground text-sm">
                Only pages and databases that have been explicitly shared with your Notion
                integration are indexed.
            </p>
            <ul class="text-muted-foreground list-disc space-y-1 pl-5 text-sm">
                <li>
                    Open each Notion page or teamspace to index, click
                    <span class="font-medium">… → Add connections</span>, and add this integration.
                    Sharing inherits to child pages.
                </li>
                <li>
                    On the integration's capabilities page, enable
                    <span class="font-medium">Read content</span>, and under
                    <span class="font-medium">User capabilities</span> select
                    <span class="font-medium"> User information with email addresses </span>.
                    Without it, listing workspace members returns 403 and group membership stays
                    empty.
                </li>
            </ul>
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
