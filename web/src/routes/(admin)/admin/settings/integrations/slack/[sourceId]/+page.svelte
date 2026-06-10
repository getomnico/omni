<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Alert from '$lib/components/ui/alert'
    import * as Card from '$lib/components/ui/card'
    import { AlertCircle, Loader2 } from '@lucide/svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import slackLogo from '$lib/images/icons/slack.svg'

    let { data }: PageProps = $props()

    let enabled = $state(data.source.isActive)
    let botToken = $state('')
    let appToken = $state('')

    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
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
        hasUnsavedChanges =
            enabled !== originalEnabled || botToken.trim() !== '' || appToken.trim() !== ''
    })

    function validateForm(): boolean {
        formErrors = []

        const trimmedBotToken = botToken.trim()
        const trimmedAppToken = appToken.trim()

        if (trimmedBotToken && !trimmedBotToken.startsWith('xoxb-')) {
            formErrors = [...formErrors, 'Bot token must start with xoxb-']
        }

        if (trimmedAppToken && !trimmedAppToken.startsWith('xapp-')) {
            formErrors = [...formErrors, 'App-Level Token must start with xapp-']
        }

        return formErrors.length === 0
    }
</script>

<svelte:head>
    <title>Configure Slack - {data.source.name}</title>
</svelte:head>

{#if formErrors.length > 0}
    <Alert.Root variant="destructive">
        <AlertCircle class="h-4 w-4" />
        <Alert.Title>Configuration Error</Alert.Title>
        <Alert.Description>
            <ul class="list-inside list-disc">
                {#each formErrors as err}
                    <li>{err}</li>
                {/each}
            </ul>
        </Alert.Description>
    </Alert.Root>
{/if}

<form
    method="POST"
    use:enhance={({ cancel }) => {
        if (!validateForm()) {
            cancel()
            return
        }

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
                        <img src={slackLogo} alt="Slack" class="h-5 w-5" />
                        {data.source.name}
                    </Card.Title>
                    <Card.Description class="mt-1">
                        Index messages and files from Slack channels
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

        <Card.Content class="space-y-6">
            <p class="text-muted-foreground text-sm">
                All public channels the bot has been added to will be indexed.
            </p>

            <div class="space-y-4 border-t pt-4">
                <div>
                    <h3 class="text-sm font-semibold">Credentials</h3>
                    <p class="text-muted-foreground mt-1 text-sm">
                        Leave token fields blank to keep the current saved values. Fill either field
                        to update only that Slack token.
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="bot-token">Bot Token</Label>
                    <Input
                        id="bot-token"
                        name="botToken"
                        bind:value={botToken}
                        placeholder="xoxb-..."
                        type="password"
                        autocomplete="new-password" />
                    <p class="text-muted-foreground text-sm">
                        Create a Slack app and get a bot token at <a
                            href="https://api.slack.com/apps"
                            target="_blank"
                            rel="noreferrer"
                            class="text-blue-600 hover:underline">api.slack.com/apps</a>
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="app-token">App-Level Token (optional)</Label>
                    <Input
                        id="app-token"
                        name="appToken"
                        bind:value={appToken}
                        placeholder="xapp-..."
                        type="password"
                        autocomplete="new-password" />
                    <p class="text-muted-foreground text-sm">
                        Enables realtime updates via Socket Mode. Generate one under your Slack App
                        &rarr; Settings &rarr; Basic Information &rarr; App-Level Tokens with the
                        <code class="bg-muted rounded px-1">connections:write</code> scope.
                    </p>
                </div>
            </div>
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
