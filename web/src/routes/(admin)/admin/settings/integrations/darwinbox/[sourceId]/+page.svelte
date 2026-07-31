<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import * as Card from '$lib/components/ui/card'
    import { DARWINBOX_EMPLOYEE_FIELDS, availableActions } from '$lib/darwinbox-config'
    import type { DarwinboxSourceConfig } from '$lib/types'
    import type { PageProps } from './$types'

    let { data, form }: PageProps = $props()
    const config = data.source.config as DarwinboxSourceConfig
    const authorization = config.authorization ?? {}
    const scope = config.employee_scope ?? { mode: 'all' }
    let readOnly = $state(config.read_only !== false)
    let writeAcknowledged = $state(Boolean(authorization.write_acknowledged))
    let participantMode = $state<'all' | 'allowlist'>(
        authorization.participant_mode === 'allowlist' ||
            (!authorization.participant_mode && (authorization.participant_emails?.length ?? 0) > 0)
            ? 'allowlist'
            : 'all',
    )
    let scopeMode = $state<'all' | 'include'>(scope.mode)
    let isSubmitting = $state(false)
    const includeScope =
        scope.mode === 'include'
            ? scope
            : { employee_ids: [], employee_emails: [], departments: [] }
    const label = (value: string) =>
        value.replaceAll('_', ' ').replace(/\b\w/g, (character) => character.toUpperCase())
    const groups = () => {
        const result = new Map<string, ReturnType<typeof availableActions>>()
        for (const action of availableActions(data.manifest).filter(
            (item) => !readOnly || item.mode === 'read',
        )) {
            const group = action.module || 'directory'
            result.set(group, [...(result.get(group) ?? []), action])
        }
        return [...result.entries()]
    }
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
                >Only capabilities advertised by the connector can be saved.</Card.Description
            ></Card.Header>
        <Card.Content class="space-y-5">
            {#if form?.message}<div
                    class="rounded border border-red-300 p-3 text-sm whitespace-pre-line text-red-700">
                    {form.message}
                </div>{/if}
            <fieldset class="space-y-2">
                <legend class="font-medium">Sync capabilities</legend>
                {#each data.manifest.sync_capabilities?.filter((item) => item.available) ?? [] as capability}
                    <label class="flex cursor-pointer items-start gap-2"
                        ><input
                            type="checkbox"
                            name="sync_modules"
                            value={capability.name}
                            checked={config.sync_modules?.[capability.name] === true} /><span
                            >{capability.name === 'employee_directory'
                                ? 'People directory'
                                : label(capability.name)}<span
                                class="text-muted-foreground block text-xs"
                                >Requires {capability.endpoints?.join(', ')}</span
                            ></span
                        ></label>
                {/each}
            </fieldset>
            <fieldset class="space-y-2">
                <legend class="font-medium">Employee scope</legend>
                <p class="text-muted-foreground text-xs">
                    Approved fields are organization-visible in the colleague directory.
                </p>
                <label class="cursor-pointer"
                    ><input type="radio" name="scope_mode" value="all" bind:group={scopeMode} /> All employees</label>
                <label class="ml-4 cursor-pointer"
                    ><input type="radio" name="scope_mode" value="include" bind:group={scopeMode} /> Include
                    only</label>
                {#if scopeMode === 'include'}<Input
                        name="employee_ids"
                        value={includeScope.employee_ids.join(', ')}
                        placeholder="Employee IDs" /><Input
                        name="employee_emails"
                        value={includeScope.employee_emails.join(', ')}
                        placeholder="Employee emails" /><Input
                        name="departments"
                        value={includeScope.departments.join(', ')}
                        placeholder="Departments" />{/if}
                <div class="grid grid-cols-2 gap-2">
                    {#each DARWINBOX_EMPLOYEE_FIELDS as field}<label class="cursor-pointer"
                            ><input
                                type="checkbox"
                                name="employee_fields"
                                value={field}
                                checked={config.employee_fields?.includes(field)} />
                            {label(field)}</label
                        >{/each}
                </div>
            </fieldset>
            <fieldset class="space-y-2">
                <legend class="font-medium">Actions</legend
                >{#each groups() as [module, actions]}<div class="font-medium">{label(module)}</div>
                    {#each actions as action}<label class="flex cursor-pointer items-start gap-2"
                            ><input
                                type="checkbox"
                                name="allowed_actions"
                                value={action.name}
                                checked={authorization.allowed_actions?.includes(
                                    action.name,
                                )} /><span
                                >{label(action.name)}<span
                                    class="text-muted-foreground block text-xs"
                                    >Requires {action.endpoints.join(', ')}</span
                                ></span
                            ></label
                        >{/each}{/each}
            </fieldset>
            <label class="flex cursor-pointer gap-2"
                ><input name="read_only" type="checkbox" bind:checked={readOnly} /> Read-only mode</label>
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
            </fieldset>
            {#if !readOnly}<label
                    class="flex cursor-pointer gap-2 rounded border border-amber-300 p-3"
                    ><input
                        name="write_acknowledged"
                        type="checkbox"
                        bind:checked={writeAcknowledged} /> I acknowledge selected write actions can change
                    production data.</label
                >{/if}
        </Card.Content><Card.Footer class="justify-end"
            ><Button type="submit" disabled={isSubmitting} class="cursor-pointer"
                >Save configuration</Button
            ></Card.Footer>
    </Card.Root>
</form>
