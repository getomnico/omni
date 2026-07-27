<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Card from '$lib/components/ui/card'
    import type { DarwinboxSourceConfig } from '$lib/types'
    import type { PageProps } from './$types'

    let { data }: PageProps = $props()
    const config = data.source.config as DarwinboxSourceConfig
    const authorization = config.authorization ?? {}
    let readOnly = $state(config.read_only !== false)
    let writeAcknowledged = $state(Boolean(authorization.write_acknowledged))
    let isSubmitting = $state(false)
</script>

<svelte:head><title>Configure Darwinbox - {data.source.name}</title></svelte:head>

<form
    method="POST"
    use:enhance={() => {
        isSubmitting = true
        return async ({ update }) => {
            await update()
            isSubmitting = false
        }
    }}>
    <div class="space-y-4">
        <Card.Root>
            <Card.Header>
                <Card.Title>{data.source.name}</Card.Title>
                <Card.Description>Manage the fail-closed production policy.</Card.Description>
            </Card.Header>
            <Card.Content class="space-y-4">
                <label class="flex cursor-pointer items-center gap-2">
                    <input name="read_only" type="checkbox" bind:checked={readOnly} />
                    <span>Read-only mode (prevents all mutations)</span>
                </label>
                <div class="space-y-1">
                    <Label for="participants">Approved participant emails</Label><Input
                        id="participants"
                        name="participant_emails"
                        value={(authorization.participant_emails ?? []).join(', ')} />
                </div>
                <div class="space-y-1">
                    <Label for="target-ids">Approved target employee IDs</Label><Input
                        id="target-ids"
                        name="target_employee_ids"
                        value={(authorization.target_employee_ids ?? []).join(', ')} />
                </div>
                <div class="space-y-1">
                    <Label for="target-emails">Approved target employee emails</Label><Input
                        id="target-emails"
                        name="target_employee_emails"
                        value={(authorization.target_employee_emails ?? []).join(', ')} />
                </div>
                <div class="space-y-1">
                    <Label for="target-departments">Approved target departments</Label><Input
                        id="target-departments"
                        name="target_departments"
                        value={(authorization.target_departments ?? []).join(', ')} />
                </div>
                {#if !readOnly}
                    <label
                        class="flex cursor-pointer items-start gap-2 rounded border border-amber-300 p-3">
                        <input
                            name="write_acknowledged"
                            type="checkbox"
                            bind:checked={writeAcknowledged} />
                        <span
                            >Allow explicitly confirmed writes only for the participants and targets
                            above.</span>
                    </label>
                {/if}
            </Card.Content>
            <Card.Footer class="justify-end"
                ><Button type="submit" disabled={isSubmitting} class="cursor-pointer"
                    >Save configuration</Button
                ></Card.Footer>
        </Card.Root>
    </div>
</form>
