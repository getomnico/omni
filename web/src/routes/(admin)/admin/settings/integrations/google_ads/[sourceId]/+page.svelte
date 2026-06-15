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
    import googleAdsLogo from '$lib/images/icons/google-ads.svg'

    let { data }: PageProps = $props()

    let enabled = $state(data.source.isActive)
    let isSubmitting = $state(false)
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)
    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null
    let originalEnabled = data.source.isActive

    const config = $derived(
        data.source.config as { customer_ids?: string[]; login_customer_id?: string },
    )
    const customerIds = $derived(config.customer_ids ?? [])

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
    <title>Configure Google Ads - {data.source.name}</title>
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
                        <img src={googleAdsLogo} alt="Google Ads" class="h-5 w-5" />
                        {data.source.name}
                    </Card.Title>
                    <Card.Description class="mt-1">
                        Index Google Ads account structure and use live reporting actions for
                        performance data.
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

        <Card.Content class="space-y-4">
            <div class="space-y-1">
                <div class="text-sm font-medium">Customer IDs</div>
                <p class="text-muted-foreground text-sm">
                    {customerIds.length > 0 ? customerIds.join(', ') : 'No customer IDs configured'}
                </p>
            </div>

            {#if config.login_customer_id}
                <div class="space-y-1">
                    <div class="text-sm font-medium">Manager/login customer ID</div>
                    <p class="text-muted-foreground text-sm">{config.login_customer_id}</p>
                </div>
            {/if}

            <p class="text-muted-foreground text-sm">
                Numeric metrics like clicks, impressions, cost, and conversions are not synced into
                the index. Use Google Ads actions to fetch performance data live.
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
