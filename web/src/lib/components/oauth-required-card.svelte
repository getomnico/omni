<script lang="ts">
    import { onMount } from 'svelte'
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import { getSourceIconPath, getSourceDisplayName } from '$lib/utils/icons.js'
    import { SourceType } from '$lib/types.js'
    import type { OAuthRequired } from '$lib/types/message.js'

    type Props = {
        oauthRequired: OAuthRequired
        toolName: string
        isAdmin: boolean
        onComplete: () => void
    }

    let { oauthRequired, toolName, isAdmin, onComplete }: Props = $props()

    // Refresh path: the persisted envelope doesn't carry providerConfigured,
    // so the +page.svelte loader defaults it to true. Re-check in the
    // background so the admin-not-configured case still surfaces the
    // disabled-button UI even after a page refresh.
    let effectiveProviderConfigured = $state(oauthRequired.providerConfigured)
    $effect(() => {
        effectiveProviderConfigured = oauthRequired.providerConfigured
        if (oauthRequired.providerConfigured) {
            void fetch(
                `/api/oauth/provider-status?provider=${encodeURIComponent(oauthRequired.provider)}`,
            )
                .then((r) => (r.ok ? r.json() : null))
                .then((body) => {
                    if (body && body.configured === false) {
                        effectiveProviderConfigured = false
                    }
                })
                .catch(() => {})
        }
    })

    const actionLabel = $derived.by(() => {
        const parts = toolName.split('__')
        return parts.length > 1 ? parts.slice(1).join('__').replaceAll('_', ' ') : toolName
    })

    const iconPath = $derived(getSourceIconPath(oauthRequired.sourceType))
    const displayName = $derived(
        oauthRequired.sourceDisplayName ||
            getSourceDisplayName(oauthRequired.sourceType as SourceType) ||
            oauthRequired.sourceType,
    )

    // Listen for OAuth completion via the existing BroadcastChannel that the
    // /oauth/done page posts to. Same channel powers the existing
    // google-oauth-setup flow, so we get cross-tab signalling for free.
    onMount(() => {
        let ch: BroadcastChannel | null = null
        try {
            ch = new BroadcastChannel('omni-user-auth')
            ch.onmessage = (event) => {
                const data = event.data
                if (
                    data &&
                    typeof data === 'object' &&
                    data.type === 'omni:user-auth-result' &&
                    data.ok === true &&
                    data.sourceId === oauthRequired.sourceId
                ) {
                    onComplete()
                }
            }
        } catch {
            // BroadcastChannel unavailable — connection completion will be
            // surfaced after a manual refresh.
        }
        return () => {
            ch?.close()
        }
    })

    function startConnect() {
        const popup = window.open(oauthRequired.oauthStartUrl, 'omni_oauth', 'width=560,height=720')
        if (!popup) {
            // Popup blocked — fall back to full-page redirect.
            window.location.href = oauthRequired.oauthStartUrl
        }
    }
</script>

<Card.Root class="gap-0 overflow-hidden py-0">
    <Card.Header class="flex items-center gap-3 border-b px-5 py-3 [.border-b]:py-3">
        {#if iconPath}
            <img src={iconPath} alt={displayName} class="h-7 w-7" />
        {/if}
        <div class="min-w-0 flex-1">
            <Card.Title class="text-sm">{displayName}</Card.Title>
            <Card.Description class="text-xs">{actionLabel}</Card.Description>
        </div>
        <div
            class="flex items-center gap-1.5 rounded-full bg-blue-100 px-2.5 py-1 dark:bg-blue-950">
            <span class="h-1.5 w-1.5 rounded-full bg-blue-500"></span>
            <span class="text-[11px] font-medium text-blue-700 dark:text-blue-400">
                Connection required
            </span>
        </div>
    </Card.Header>

    <Card.Content class="px-5 py-4 text-[13px]">
        {#if effectiveProviderConfigured}
            <p class="text-muted-foreground">
                This action needs your authorization for {displayName}. Connect your account to
                continue.
            </p>
        {:else}
            <p class="text-muted-foreground">
                An admin must finish setting up the {displayName} integration before this action can run.
            </p>
            {#if isAdmin}
                <p class="mt-2">
                    <a
                        href="/settings/integrations"
                        class="text-primary underline underline-offset-2">
                        Configure {displayName}
                    </a>
                </p>
            {/if}
        {/if}
    </Card.Content>

    {#if effectiveProviderConfigured}
        <Card.Footer class="bg-muted/50 justify-end gap-2 border-t px-3 py-3 [.border-t]:py-3">
            <Button size="sm" variant="default" class="cursor-pointer" onclick={startConnect}>
                Connect {displayName}
            </Button>
        </Card.Footer>
    {/if}
</Card.Root>
