<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { toast } from 'svelte-sonner'
    import googleDriveLogo from '$lib/images/icons/google-drive.svg'
    import gmailLogo from '$lib/images/icons/gmail.svg'

    interface Props {
        open: boolean
        connectedSourceTypes?: string[]
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, connectedSourceTypes = [], onSuccess, onCancel }: Props = $props()

    let driveAlreadyConnected = $derived(connectedSourceTypes.includes('google_drive'))
    let gmailAlreadyConnected = $derived(connectedSourceTypes.includes('gmail'))

    let connectDrive = $state(true)
    let connectGmail = $state(true)
    let isSubmitting = $state(false)

    async function handleConnect() {
        if (!connectDrive && !connectGmail) {
            toast.error('Please select at least one service to connect')
            return
        }

        isSubmitting = true

        const serviceTypes = []
        if (connectDrive) serviceTypes.push('google_drive')
        if (connectGmail) serviceTypes.push('gmail')

        window.location.href = `/api/oauth/start?source_types=${serviceTypes.join(',')}`
    }

    function handleCancel() {
        connectDrive = true
        connectGmail = true
        onCancel?.()
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-md">
        <Dialog.Header>
            <Dialog.Title>Connect with Google</Dialog.Title>
            <Dialog.Description>
                Choose which Google services to connect. You'll be redirected to Google to authorize
                access.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4 py-4">
            <label
                class="flex items-center gap-3 rounded-lg border p-3 {driveAlreadyConnected
                    ? 'opacity-60'
                    : 'hover:bg-muted/50 cursor-pointer'}">
                <Checkbox bind:checked={connectDrive} disabled={driveAlreadyConnected} />
                <img src={googleDriveLogo} alt="Google Drive" class="h-5 w-5" />
                <div class="flex-1">
                    <div class="flex items-center gap-2">
                        <span class="font-medium">Google Drive</span>
                        {#if driveAlreadyConnected}
                            <span
                                class="inline-flex items-center rounded-full bg-green-100 px-1.5 py-0.5 text-[10px] font-medium text-green-800 dark:bg-green-900/20 dark:text-green-400">
                                Already connected
                            </span>
                        {/if}
                    </div>
                    <div class="text-muted-foreground text-sm">
                        Index your Drive documents, spreadsheets, and presentations
                    </div>
                </div>
            </label>

            <label
                class="flex items-center gap-3 rounded-lg border p-3 {gmailAlreadyConnected
                    ? 'opacity-60'
                    : 'hover:bg-muted/50 cursor-pointer'}">
                <Checkbox bind:checked={connectGmail} disabled={gmailAlreadyConnected} />
                <img src={gmailLogo} alt="Gmail" class="h-5 w-5" />
                <div class="flex-1">
                    <div class="flex items-center gap-2">
                        <span class="font-medium">Gmail</span>
                        {#if gmailAlreadyConnected}
                            <span
                                class="inline-flex items-center rounded-full bg-green-100 px-1.5 py-0.5 text-[10px] font-medium text-green-800 dark:bg-green-900/20 dark:text-green-400">
                                Already connected
                            </span>
                        {/if}
                    </div>
                    <div class="text-muted-foreground text-sm">
                        Index your email threads and conversations
                    </div>
                </div>
            </label>

            <p class="text-muted-foreground text-xs">
                Only your own data will be synced. Omni will have read-only access.
            </p>
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button
                onclick={handleConnect}
                disabled={isSubmitting ||
                    (!connectDrive && !connectGmail) ||
                    (driveAlreadyConnected && gmailAlreadyConnected)}
                class="cursor-pointer">
                {isSubmitting ? 'Connecting...' : 'Connect with Google'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
