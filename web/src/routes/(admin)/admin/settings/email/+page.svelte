<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Card from '$lib/components/ui/card'
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import * as Dialog from '$lib/components/ui/dialog'
    import { CheckCircle2, Loader2, Pencil, Trash2, Server, Zap, Mail, Send } from '@lucide/svelte'
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

    let isTesting = $state(false)
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

        <!-- Provider Cards -->
        <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
            {#each EMAIL_PROVIDER_TYPES as type}
                {@const provider = providerByType[type]}
                {@const meta = providerMeta[type]}
                <Card.Root>
                    <Card.Header class="flex flex-row items-start justify-between space-y-0 pb-2">
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
                                <Card.Title class="text-lg">
                                    {EMAIL_PROVIDER_LABELS[type]}
                                </Card.Title>
                                {#if provider}
                                    <div class="flex items-center gap-1.5 text-sm text-green-600">
                                        <CheckCircle2 class="h-3.5 w-3.5" />
                                        Connected
                                        {#if provider.isCurrent}
                                            <span
                                                class="bg-primary/10 text-primary ml-1 rounded-full px-1.5 py-0.5 text-xs">
                                                Current
                                            </span>
                                        {/if}
                                    </div>
                                {:else}
                                    <Card.Description>{meta.description}</Card.Description>
                                {/if}
                            </div>
                        </div>
                    </Card.Header>
                    <Card.Content>
                        {#if provider}
                            {@const config = provider.config as Record<string, string>}
                            <div class="mb-3 space-y-1">
                                <p class="text-muted-foreground text-xs font-medium uppercase">
                                    Configuration
                                </p>
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

                            <div class="flex flex-wrap gap-2">
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
                                <Button
                                    variant="outline"
                                    size="sm"
                                    class="cursor-pointer gap-1"
                                    onclick={() => openEditDialog(provider)}>
                                    <Pencil class="h-3 w-3" />
                                    Edit
                                </Button>
                                <form
                                    method="POST"
                                    action="?/delete"
                                    use:enhance={enhanceWithToast}>
                                    <input type="hidden" name="id" value={provider.id} />
                                    <Button
                                        type="button"
                                        variant="outline"
                                        size="sm"
                                        class="cursor-pointer gap-1 text-red-600 hover:text-red-700"
                                        onclick={(e) => {
                                            const form = (e.currentTarget as HTMLElement).closest(
                                                'form',
                                            )!
                                            requestConfirm(
                                                'Remove Provider',
                                                `Are you sure you want to remove "${EMAIL_PROVIDER_LABELS[type]}"?${provider.isCurrent ? ' This is the current provider — removing it will disable email sending until another provider is configured.' : ''}`,
                                                form as HTMLFormElement,
                                            )
                                        }}>
                                        <Trash2 class="h-3 w-3" />
                                        Remove
                                    </Button>
                                </form>
                            </div>
                        {:else}
                            <Button
                                class="mt-1 cursor-pointer gap-2"
                                onclick={() => openSetupDialog(type)}>
                                Connect
                            </Button>
                        {/if}
                    </Card.Content>
                </Card.Root>
            {/each}
        </div>

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
                            <input
                                type="checkbox"
                                id="secure"
                                name="secure"
                                value="true"
                                checked={formState.secure}
                                onchange={(e) =>
                                    (formState.secure = (e.target as HTMLInputElement).checked)}
                                class="h-4 w-4" />
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
                            ? 'bg-red-600 text-white hover:bg-red-700'
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
