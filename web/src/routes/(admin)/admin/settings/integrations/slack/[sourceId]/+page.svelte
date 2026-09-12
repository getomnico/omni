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

<!-- AI tools: powered by Slack's official hosted MCP server under a per-user
     delegated OAuth identity. Bot/app tokens above only power indexing/sync. -->
<Card.Root class="mt-6">
    <Card.Header>
        <Card.Title class="flex items-center gap-2">
            <img src={slackLogo} alt="Slack" class="h-5 w-5" />
            AI tools
        </Card.Title>
        <Card.Description class="mt-1">
            Agents can search Slack, read channels and threads, and send messages on behalf of
            users. These run through Slack's official MCP server (<code
                class="bg-muted rounded px-1">mcp.slack.com</code
            >) and always act as the authenticated user — never as the sync bot.
        </Card.Description>
    </Card.Header>

    <Card.Content class="space-y-4">
        <div class="bg-muted/50 rounded-md border p-3 text-sm">
            {#if data.actionAuth.authorized}
                <p class="text-muted-foreground">
                    Connected as
                    <span class="text-foreground font-medium"
                        >{data.actionAuth.principalEmail}</span>
                    ({data.source.name}). Reconnect to switch the Slack account that populates the
                    shared tool catalog.
                </p>
            {:else}
                <p class="text-muted-foreground">
                    No Slack account is connected yet. Connect one to enable Slack AI tools for all
                    users; each user then authorizes their own Slack identity the first time they
                    use an AI tool.
                </p>
            {/if}
        </div>

        {#if !data.oauthClientConfigured}
            <Alert.Root variant="destructive">
                <AlertCircle class="h-4 w-4" />
                <Alert.Title>OAuth client not configured</Alert.Title>
                <Alert.Description>
                    The Slack OAuth client must be configured before users can authorize. Add it
                    under
                    <a href="/admin/settings/integrations" class="text-foreground underline"
                        >Admin &rarr; Settings &rarr; Integrations &rarr; OAuth Apps</a
                    >, then register this redirect URI in your Slack app:
                    <code class="bg-muted rounded px-1">{data.oauthRedirectUri}</code>
                </Alert.Description>
            </Alert.Root>
        {/if}

        <ol class="text-muted-foreground list-decimal space-y-1 pl-5 text-sm">
            <li>
                Your Slack app must have MCP server access enabled. Open
                <a
                    href="https://api.slack.com/apps"
                    target="_blank"
                    rel="noreferrer"
                    class="text-blue-600 hover:underline">api.slack.com/apps</a>
                and enable it under your app's <span class="text-foreground">App Assistant</span>
                settings.
            </li>
            <li>
                The Slack app's user token scopes must include the scopes Omni requests. Add them
                under <span class="text-foreground"
                    >OAuth &amp; Permissions &rarr; User Token Scopes</span
                >:
                <details class="mt-1">
                    <summary class="text-foreground w-fit cursor-pointer underline">
                        Show requested scopes
                    </summary>
                    <code class="bg-muted mt-1 block rounded p-2 text-xs leading-relaxed"
                        >chat:write channels:history channels:read groups:history groups:read
                        im:history im:read mpim:history mpim:read channels:write groups:write
                        im:write mpim:write reactions:write reactions:read canvases:read
                        canvases:write files:read files:write emoji:read users:read users:read.email
                        search:read.public search:read.private search:read.im search:read.mpim
                        search:read.files search:read.users lists:read lists:write</code>
                </details>
            </li>
            <li>
                Grant access to the Slack app: enter its Client ID and Client Secret under
                <a href="/admin/settings/integrations" class="text-blue-600 hover:underline"
                    >OAuth Apps</a>
                and add the redirect URI above to the app.
            </li>
        </ol>

        <div class="flex items-center gap-3 pt-1">
            <Button
                href={`/api/oauth/start?source_id=${data.source.id}&flow=user_write&return_to=${encodeURIComponent(
                    `/admin/settings/integrations/slack/${data.source.id}`,
                )}`}
                variant={data.actionAuth.authorized ? 'outline' : 'default'}
                class="cursor-pointer">
                {data.actionAuth.authorized ? 'Reconnect Slack account' : 'Connect Slack account'}
            </Button>
            {#if !data.oauthClientConfigured}
                <span class="text-muted-foreground text-xs">
                    Disabled until the OAuth client above is configured.
                </span>
            {/if}
        </div>
    </Card.Content>
</Card.Root>
