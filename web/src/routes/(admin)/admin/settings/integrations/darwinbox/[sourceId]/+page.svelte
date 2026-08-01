<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import * as Card from '$lib/components/ui/card'
    import type { DarwinboxSourceConfig } from '$lib/types'
    import type { PageProps } from './$types'

    let { data, form }: PageProps = $props()
    const config = data.source.config as DarwinboxSourceConfig
    const authorization = config.authorization ?? {}
    let readOnly = $state(config.read_only !== false)
    let participantMode = $state<'all' | 'allowlist'>(
        authorization.participant_mode === 'allowlist' ||
            (!authorization.participant_mode && (authorization.participant_emails?.length ?? 0) > 0)
            ? 'allowlist'
            : 'all',
    )
    let isSubmitting = $state(false)
</script>

<svelte:head><title>Configure Darwinbox - {data.source.name}</title></svelte:head>
<form
    method="POST"
    use:enhance={() => {
        isSubmitting = true
        return async ({ update }) => {
            await update({ reset: false })
            isSubmitting = false
        }
    }}>
    <Card.Root
        ><Card.Header
            ><Card.Title>{data.source.name}</Card.Title><Card.Description
                >The dataset key controls which users and fields the Darwinbox API can access; Omni
                indexes whatever the provider allows and skips denied modules.</Card.Description
            ></Card.Header>
        <Card.Content class="space-y-5">
            {#if form?.message}<div
                    class="rounded border border-red-300 p-3 text-sm whitespace-pre-line text-red-700">
                    {form.message}
                </div>{/if}
            <label class="flex cursor-pointer gap-2"
                ><input name="read_only" type="checkbox" bind:checked={readOnly} /> Read-only mode (prevents
                all mutations)</label>
            <fieldset class="space-y-2">
                <legend class="font-medium">Action participants</legend><label
                    class="flex cursor-pointer gap-2"
                    ><input
                        type="radio"
                        name="participant_mode"
                        value="all"
                        bind:group={participantMode} /> Everyone (default)</label
                ><label class="flex cursor-pointer gap-2"
                    ><input
                        type="radio"
                        name="participant_mode"
                        value="allowlist"
                        bind:group={participantMode} /> Only specific people</label
                >{#if participantMode === 'allowlist'}<Input
                        name="participant_emails"
                        value={(authorization.participant_emails ?? []).join(', ')}
                        placeholder="Approved participant emails (comma-separated)" />
                {/if}
                <p class="text-muted-foreground text-xs">
                    Leave as "Everyone" to let any authenticated organization member invoke
                    Darwinbox actions.
                </p>
            </fieldset>
        </Card.Content><Card.Footer class="justify-end"
            ><Button type="submit" disabled={isSubmitting} class="cursor-pointer"
                >Save configuration</Button
            ></Card.Footer>
    </Card.Root>
</form>
