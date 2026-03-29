<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { Badge } from '$lib/components/ui/badge'
    import * as Card from '$lib/components/ui/card'
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import * as Dialog from '$lib/components/ui/dialog'
    import { Loader2, Pencil, Trash2, Server, Zap, Mail, Send } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageData } from './$types'
    import { EMAIL_PROVIDER_TYPES, EMAIL_PROVIDER_LABELS, type EmailProviderType } from '$lib/types'
    import azureIcon from '$lib/images/icons/azure.svg'

    let { data }: { data: PageData } = $props()

    interface ProviderFormState {
        id?: string
        providerType: EmailProviderType
        connectionString: string
        senderAddress: string
        apiKey: string
        fromEmail: string
        host: string
        port: string
        user: string
        password: string
        secure: boolean
    }

    const emptyForm: ProviderFormState = {
        providerType: 'acs',
        connectionString: '',
        senderAddress: '',
        apiKey: '',
        fromEmail: '',
        host: '',
        port: '',
        user: '',
        password: '',
        secure: false,
    }

    let dialogOpen = $state(false)
    let editMode = $state(false)
    let formState = $state<ProviderFormState>({ ...emptyForm })
    let isSubmitting = $state(false)
    let editingHasSecret = $state(false)

    let confirmDialogOpen = $state(false)
    let confirmTitle = $state('')
    let confirmDescription = $state('')
    let confirmFormRef = $state<HTMLFormElement | null>(null)
    let confirmActionLabel = $state('Confirm')
    let confirmDestructive = $state(true)

    function requestConfirm(
        title: string,
        description: string,
        formEl: HTMLFormElement,
        actionLabel = 'Remove',
        destructive = true,
    ) {
        confirmTitle = title
        confirmDescription = description
        confirmFormRef = formEl
        confirmActionLabel = actionLabel
        confirmDestructive = destructive
        confirmDialogOpen = true
    }

    const providerMeta: Record<EmailProviderType, { description: string; icon: string | null }> = {
        acs: {
            description: 'Send emails via Azure Communication Services',
            icon: azureIcon,
        },
        resend: {
            description: 'Send emails via the Resend API',
            icon: null,
        },
        smtp: {
            description: 'Send emails via any SMTP server',
            icon: null,
        },
    }

    function enhanceWithToast() {
        return async ({ result, update }: { result: any; update: () => Promise<void> }) => {
            await update()
            confirmDialogOpen = false
            if (result.type === 'success') {
                toast.success(result.data?.message || 'Operation completed successfully')
            } else if (result.type === 'failure') {
                toast.error(result.data?.error || 'Something went wrong')
            }
        }
    }

    let providerByType = $derived(
        Object.fromEntries(
            EMAIL_PROVIDER_TYPES.map((t) => [
                t,
                data.providers.find((p) => p.providerType === t) ?? null,
            ]),
        ) as Record<EmailProviderType, (typeof data.providers)[0] | null>,
    )

    let connectedProviders = $derived(
        EMAIL_PROVIDER_TYPES.filter((t) => providerByType[t] !== null).map((t) => ({
            type: t,
            provider: providerByType[t]!,
            meta: providerMeta[t],
        })),
    )

    let unconfiguredTypes = $derived(EMAIL_PROVIDER_TYPES.filter((t) => providerByType[t] === null))

    function openSetupDialog(type: EmailProviderType) {
        editMode = false
        editingHasSecret = false
        formState = {
            ...emptyForm,
            providerType: type,
        }
        dialogOpen = true
    }

    function openEditDialog(provider: (typeof data.providers)[0]) {
        editMode = true
        editingHasSecret = provider.hasSecret
        const config = provider.config as Record<string, string>
        formState = {
            id: provider.id,
            providerType: provider.providerType as EmailProviderType,
            connectionString: '',
            senderAddress: config.senderAddress || '',
            apiKey: '',
            fromEmail: config.fromEmail || '',
            host: config.host || '',
            port: config.port || '',
            user: '',
            password: '',
            secure: String(config.secure) === 'true',
        }
        dialogOpen = true
    }

    let isSendingTest = $state(false)
</script>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Email Provider</h1>
            <p class="text-muted-foreground mt-2">
                Configure an email provider to enable transactional emails (login links,
                notifications). Only one provider can be active at a time.
            </p>
        </div>

        <!-- Connected Provider Cards -->
        {#if connectedProviders.length > 0}
            <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
                {#each connectedProviders as { type, provider, meta } (provider.id)}
                    <Card.Root class="group/card">
                        <Card.Header class="pb-2">
                            <div class="flex items-start gap-3">
                                {#if meta.icon}
                                    <img
                                        src={meta.icon}
                                        alt={EMAIL_PROVIDER_LABELS[type]}
                                        class="h-8 w-8" />
                                {:else if type === 'smtp'}
                                    <Server class="text-muted-foreground h-8 w-8" />
                                {:else}
                                    <Mail class="text-muted-foreground h-8 w-8" />
                                {/if}
                                <div>
                                    <div class="text-base leading-tight font-semibold">
                                        {EMAIL_PROVIDER_LABELS[type]}
                                    </div>
                                    <div class="mt-1 flex items-center gap-1.5">
                                        <Badge
                                            variant="secondary"
                                            class="border-green-200 bg-green-50 text-green-700 dark:border-green-800 dark:bg-green-950 dark:text-green-400">
                                            <span
                                                class="mr-1 inline-block h-1.5 w-1.5 rounded-full bg-green-500"
                                            ></span>
                                            Connected
                                        </Badge>
                                        {#if provider.isCurrent}
                                            <Badge variant="default">Current</Badge>
                                        {/if}
                                    </div>
                                </div>
                            </div>
                            <Card.Action>
                                <div
                                    class="flex items-center gap-1 opacity-0 transition-opacity group-hover/card:opacity-100">
                                    <Button
                                        variant="ghost"
                                        size="icon"
                                        class="h-8 w-8 cursor-pointer"
                                        title="Edit provider"
                                        onclick={() => openEditDialog(provider)}>
                                        <Pencil class="h-4 w-4" />
                                    </Button>
                                    <form
                                        method="POST"
                                        action="?/delete"
                                        use:enhance={enhanceWithToast}>
                                        <input type="hidden" name="id" value={provider.id} />
                                        <Button
                                            variant="ghost"
                                            size="icon"
                                            class="hover:text-destructive h-8 w-8 cursor-pointer"
                                            title="Remove provider"
                                            onclick={(e) => {
                                                const form = (
                                                    e.currentTarget as HTMLElement
                                                ).closest('form')!
                                                requestConfirm(
                                                    'Remove Provider',
                                                    `Are you sure you want to remove "${EMAIL_PROVIDER_LABELS[type]}"?${provider.isCurrent ? ' This is the current provider — removing it will disable email sending until another provider is configured.' : ''}`,
                                                    form as HTMLFormElement,
                                                )
                                            }}>
                                            <Trash2 class="h-4 w-4" />
                                        </Button>
                                    </form>
                                </div>
                            </Card.Action>
                        </Card.Header>
                        <Card.Content>
                            {@const config = provider.config as Record<string, string>}
                            <div class="space-y-1">
                                <span
                                    class="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
                                    Configuration
                                </span>
                                {#if type === 'acs'}
                                    <div class="text-sm">
                                        <span class="text-muted-foreground">Sender:</span>
                                        {config.senderAddress || 'Not set'}
                                    </div>
                                {:else if type === 'resend'}
                                    <div class="text-sm">
                                        <span class="text-muted-foreground">From:</span>
                                        {config.fromEmail || 'Not set'}
                                    </div>
                                {:else}
                                    <div class="text-sm">
                                        <span class="text-muted-foreground">Host:</span>
                                        {config.host || 'Not set'}
                                    </div>
                                    <div class="text-sm">
                                        <span class="text-muted-foreground">From:</span>
                                        {config.fromEmail || 'Not set'}
                                    </div>
                                {/if}
                            </div>

                            <div class="mt-3 flex flex-wrap gap-2">
                                {#if !provider.isCurrent}
                                    <form
                                        method="POST"
                                        action="?/setCurrent"
                                        use:enhance={enhanceWithToast}>
                                        <input type="hidden" name="id" value={provider.id} />
                                        <Button
                                            type="button"
                                            variant="outline"
                                            size="sm"
                                            class="cursor-pointer gap-1"
                                            onclick={(e) => {
                                                const form = (
                                                    e.currentTarget as HTMLElement
                                                ).closest('form')!
                                                requestConfirm(
                                                    'Switch Email Provider',
                                                    `This will switch the active email provider to "${EMAIL_PROVIDER_LABELS[type]}". All future emails will be sent through this provider.`,
                                                    form as HTMLFormElement,
                                                    'Switch',
                                                    false,
                                                )
                                            }}>
                                            <Zap class="h-3 w-3" />
                                            Set as Current
                                        </Button>
                                    </form>
                                {/if}
                                {#if provider.isCurrent}
                                    <form
                                        method="POST"
                                        action="?/sendTest"
                                        use:enhance={() => {
                                            isSendingTest = true
                                            return async ({ result, update }) => {
                                                isSendingTest = false
                                                if (result.type === 'success') {
                                                    toast.success(
                                                        result.data?.message || 'Test email sent',
                                                    )
                                                } else if (result.type === 'failure') {
                                                    toast.error(
                                                        result.data?.error ||
                                                            'Failed to send test email',
                                                    )
                                                }
                                            }
                                        }}>
                                        <Button
                                            type="submit"
                                            variant="outline"
                                            size="sm"
                                            disabled={isSendingTest}
                                            class="cursor-pointer gap-1">
                                            {#if isSendingTest}
                                                <Loader2 class="h-3 w-3 animate-spin" />
                                                Sending...
                                            {:else}
                                                <Send class="h-3 w-3" />
                                                Send Test Email
                                            {/if}
                                        </Button>
                                    </form>
                                {/if}
                            </div>
                        </Card.Content>
                    </Card.Root>
                {/each}
            </div>
        {/if}

        <!-- Connect a Provider -->
        {#if unconfiguredTypes.length > 0}
            <div class="space-y-3">
                <h2 class="text-lg font-semibold">Connect a Provider</h2>
                <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
                    {#each unconfiguredTypes as type}
                        {@const meta = providerMeta[type]}
                        <button
                            type="button"
                            class="cursor-pointer text-left"
                            onclick={() => openSetupDialog(type)}>
                            <Card.Root
                                class="hover:border-foreground/20 hover:bg-accent/50 h-full transition-colors">
                                <Card.Header>
                                    <div class="flex items-center gap-3">
                                        {#if meta.icon}
                                            <img
                                                src={meta.icon}
                                                alt={EMAIL_PROVIDER_LABELS[type]}
                                                class="h-8 w-8" />
                                        {:else if type === 'smtp'}
                                            <Server class="text-muted-foreground h-8 w-8" />
                                        {:else}
                                            <Mail class="text-muted-foreground h-8 w-8" />
                                        {/if}
                                        <div>
                                            <Card.Title class="text-sm">
                                                {EMAIL_PROVIDER_LABELS[type]}
                                            </Card.Title>
                                            <Card.Description class="text-xs">
                                                {meta.description}
                                            </Card.Description>
                                        </div>
                                    </div>
                                </Card.Header>
                            </Card.Root>
                        </button>
                    {/each}
                </div>
            </div>
        {/if}

        <!-- Provider Setup / Edit Dialog -->
        <Dialog.Root bind:open={dialogOpen}>
            <Dialog.Content class="max-h-[90vh] overflow-y-auto sm:max-w-lg">
                <Dialog.Header>
                    <Dialog.Title>
                        {editMode ? 'Edit' : 'Connect'}
                        {EMAIL_PROVIDER_LABELS[formState.providerType]}
                    </Dialog.Title>
                    <Dialog.Description>
                        {editMode
                            ? 'Update the email provider configuration'
                            : providerMeta[formState.providerType].description}
                    </Dialog.Description>
                </Dialog.Header>

                <form
                    method="POST"
                    action={editMode ? '?/edit' : '?/add'}
                    use:enhance={() => {
                        isSubmitting = true
                        return async ({ result, update }) => {
                            await update()
                            isSubmitting = false
                            dialogOpen = false
                            if (result.type === 'success') {
                                toast.success(
                                    result.data?.message || 'Operation completed successfully',
                                )
                            } else if (result.type === 'failure') {
                                toast.error(result.data?.error || 'Something went wrong')
                            }
                        }
                    }}
                    class="space-y-4">
                    {#if editMode}
                        <input type="hidden" name="id" value={formState.id} />
                    {/if}
                    <input type="hidden" name="providerType" value={formState.providerType} />

                    <!-- ACS fields -->
                    {#if formState.providerType === 'acs'}
                        <div class="space-y-2">
                            <Label for="connectionString">
                                Connection String {editingHasSecret && editMode ? '' : '*'}
                            </Label>
                            <Input
                                id="connectionString"
                                name="connectionString"
                                type="password"
                                bind:value={formState.connectionString}
                                placeholder={editingHasSecret && editMode
                                    ? 'Leave empty to keep current'
                                    : 'endpoint=https://...;accesskey=...'}
                                required={!editMode} />
                        </div>
                        <div class="space-y-2">
                            <Label for="senderAddress">Sender Address *</Label>
                            <Input
                                id="senderAddress"
                                name="senderAddress"
                                type="email"
                                bind:value={formState.senderAddress}
                                placeholder="omni@mail.yourdomain.com"
                                required />
                        </div>
                    {/if}

                    <!-- Resend fields -->
                    {#if formState.providerType === 'resend'}
                        <div class="space-y-2">
                            <Label for="apiKey">
                                API Key {editingHasSecret && editMode ? '' : '*'}
                            </Label>
                            <Input
                                id="apiKey"
                                name="apiKey"
                                type="password"
                                bind:value={formState.apiKey}
                                placeholder={editingHasSecret && editMode
                                    ? 'Leave empty to keep current key'
                                    : 're_...'}
                                required={!editMode} />
                        </div>
                        <div class="space-y-2">
                            <Label for="fromEmail">From Email *</Label>
                            <Input
                                id="fromEmail"
                                name="fromEmail"
                                bind:value={formState.fromEmail}
                                placeholder="Omni <noreply@yourdomain.com>"
                                required />
                        </div>
                    {/if}

                    <!-- SMTP fields -->
                    {#if formState.providerType === 'smtp'}
                        <div class="space-y-2">
                            <Label for="host">SMTP Host *</Label>
                            <Input
                                id="host"
                                name="host"
                                bind:value={formState.host}
                                placeholder="smtp.example.com"
                                required />
                        </div>
                        <div class="space-y-2">
                            <Label for="port">Port</Label>
                            <Input
                                id="port"
                                name="port"
                                type="number"
                                bind:value={formState.port}
                                placeholder="587" />
                        </div>
                        <div class="space-y-2">
                            <Label for="user">
                                Username {editingHasSecret && editMode ? '' : '*'}
                            </Label>
                            <Input
                                id="user"
                                name="user"
                                bind:value={formState.user}
                                placeholder={editingHasSecret && editMode
                                    ? 'Leave empty to keep current'
                                    : 'username'}
                                required={!editMode} />
                        </div>
                        <div class="space-y-2">
                            <Label for="password">
                                Password {editingHasSecret && editMode ? '' : '*'}
                            </Label>
                            <Input
                                id="password"
                                name="password"
                                type="password"
                                bind:value={formState.password}
                                placeholder={editingHasSecret && editMode
                                    ? 'Leave empty to keep current'
                                    : 'password'}
                                required={!editMode} />
                        </div>
                        <div class="flex items-center gap-2">
                            <Checkbox
                                id="secure"
                                name="secure"
                                value="true"
                                checked={formState.secure}
                                onCheckedChange={(v) => (formState.secure = v === true)} />
                            <Label for="secure" class="font-normal">Use TLS/SSL</Label>
                        </div>
                        <div class="space-y-2">
                            <Label for="fromEmail">From Email *</Label>
                            <Input
                                id="fromEmail"
                                name="fromEmail"
                                bind:value={formState.fromEmail}
                                placeholder="Omni <noreply@yourdomain.com>"
                                required />
                        </div>
                    {/if}

                    <Dialog.Footer>
                        <Button
                            variant="outline"
                            type="button"
                            class="cursor-pointer"
                            onclick={() => (dialogOpen = false)}>
                            Cancel
                        </Button>
                        <Button type="submit" disabled={isSubmitting} class="cursor-pointer">
                            {#if isSubmitting}
                                <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                                Saving...
                            {:else}
                                {editMode ? 'Update' : 'Connect'}
                            {/if}
                        </Button>
                    </Dialog.Footer>
                </form>
            </Dialog.Content>
        </Dialog.Root>

        <!-- Confirm Dialog -->
        <AlertDialog.Root bind:open={confirmDialogOpen}>
            <AlertDialog.Content>
                <AlertDialog.Header>
                    <AlertDialog.Title>{confirmTitle}</AlertDialog.Title>
                    <AlertDialog.Description>{confirmDescription}</AlertDialog.Description>
                </AlertDialog.Header>
                <AlertDialog.Footer>
                    <AlertDialog.Cancel class="cursor-pointer">Cancel</AlertDialog.Cancel>
                    <AlertDialog.Action
                        class="cursor-pointer {confirmDestructive
                            ? 'bg-destructive text-destructive-foreground hover:bg-destructive/90'
                            : ''}"
                        onclick={() => {
                            confirmFormRef?.requestSubmit()
                        }}>
                        {confirmActionLabel}
                    </AlertDialog.Action>
                </AlertDialog.Footer>
            </AlertDialog.Content>
        </AlertDialog.Root>
    </div>
</div>
