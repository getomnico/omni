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
    const scope = config.employee_scope?.mode === 'include' ? config.employee_scope : null
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
                    <span>Read-only mode (blocks every Darwinbox mutation)</span>
                </label>
                <label class="flex cursor-pointer items-center gap-2">
                    <input
                        name="employee_directory"
                        type="checkbox"
                        checked={Boolean(config.sync_modules?.employee_directory)} />
                    <span>Index the scoped employee directory</span>
                </label>
                <div class="space-y-1">
                    <Label for="employee-ids">Employee IDs to index</Label><Input
                        id="employee-ids"
                        name="employee_ids"
                        value={(scope?.employee_ids ?? []).join(', ')} />
                </div>
                <div class="space-y-1">
                    <Label for="employee-emails">Employee emails to index</Label><Input
                        id="employee-emails"
                        name="employee_emails"
                        value={(scope?.employee_emails ?? []).join(', ')} />
                </div>
                <div class="space-y-1">
                    <Label for="departments">Departments to index</Label><Input
                        id="departments"
                        name="departments"
                        value={(scope?.departments ?? []).join(', ')} />
                </div>
                <div class="space-y-1">
                    <Label for="employee-fields">Indexed employee fields</Label>
                    <Input
                        id="employee-fields"
                        name="employee_fields"
                        value={(config.employee_fields ?? []).join(', ')} />
                    <p class="text-muted-foreground text-xs">
                        Allowed: name, employee_id, company_email, department, designation,
                        office_location, manager_employee_id, employee_type.
                    </p>
                </div>
                <label class="flex cursor-pointer items-center gap-2"
                    ><input
                        name="employee_self_service"
                        type="checkbox"
                        checked={Boolean(config.action_modules?.employee_self_service)} /><span
                        >Enable approved self-service actions</span
                    ></label>
                <label class="flex cursor-pointer items-center gap-2"
                    ><input
                        name="manager_workflows"
                        type="checkbox"
                        checked={Boolean(config.action_modules?.manager_workflows)} /><span
                        >Enable approved manager actions</span
                    ></label>
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
                <div class="space-y-1">
                    <Label for="actions">Exact allowed action names</Label>
                    <Input
                        id="actions"
                        name="allowed_actions"
                        value={(authorization.allowed_actions ?? []).join(', ')} />
                    <p class="text-muted-foreground text-xs">
                        Only reviewed self-service and direct-manager actions are accepted by server
                        validation.
                    </p>
                </div>
                <div class="grid gap-4 md:grid-cols-2">
                    <div class="space-y-1">
                        <Label for="batch">Maximum targets per action</Label><Input
                            id="batch"
                            name="max_batch_size"
                            type="number"
                            min="1"
                            max="20"
                            value={authorization.max_batch_size ?? 1} />
                    </div>
                    <div class="space-y-1">
                        <Label for="rate">Requests per minute</Label><Input
                            id="rate"
                            name="max_requests_per_minute"
                            type="number"
                            min="1"
                            max="60"
                            value={authorization.max_requests_per_minute ?? 10} />
                    </div>
                </div>
                {#if !readOnly}
                    <label
                        class="flex cursor-pointer items-start gap-2 rounded border border-amber-300 p-3">
                        <input
                            name="write_acknowledged"
                            type="checkbox"
                            bind:checked={writeAcknowledged} />
                        <span
                            >Allow explicitly confirmed writes only for the participants, targets,
                            and actions above.</span>
                    </label>
                {/if}
                <p class="text-muted-foreground text-sm">
                    Organization masters, positions, holidays, ATS, reports, HR administration, bulk
                    writes, and background writes remain unavailable until typed provider contracts
                    are reviewed.
                </p>
            </Card.Content>
            <Card.Footer class="justify-end"
                ><Button type="submit" disabled={isSubmitting} class="cursor-pointer"
                    >Save configuration</Button
                ></Card.Footer>
        </Card.Root>
        <Card.Root>
            <Card.Header
                ><Card.Title>Required Darwinbox endpoints</Card.Title><Card.Description
                    >Ask Darwinbox to authorize only this endpoint set for the integration
                    credential.</Card.Description
                ></Card.Header>
            <Card.Content class="grid gap-4 md:grid-cols-2">
                <div>
                    <div class="font-medium">Read</div>
                    <ul class="text-muted-foreground list-inside list-disc text-sm">
                        {#each data.endpoints.read as endpoint}<li>{endpoint}</li>{/each}
                    </ul>
                </div>
                <div>
                    <div class="font-medium">Write</div>
                    <ul class="text-muted-foreground list-inside list-disc text-sm">
                        {#each data.endpoints.write as endpoint}<li>{endpoint}</li>{/each}
                    </ul>
                </div>
            </Card.Content>
        </Card.Root>
    </div>
</form>
