<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { toast } from 'svelte-sonner'
    import googleDriveLogo from '$lib/images/icons/google-drive.svg'
    import gmailLogo from '$lib/images/icons/gmail.svg'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = $bindable(false), onSuccess, onCancel }: Props = $props()

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

        window.location.href = `/api/connectors/google/oauth/start?serviceTypes=${serviceTypes.join(',')}`
    }

    function handleCancel() {
        open = false
        connectDrive = true
        connectGmail = true
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root bind:open>
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
                class="hover:bg-muted/50 flex cursor-pointer items-center gap-3 rounded-lg border p-3">
                <input type="checkbox" bind:checked={connectDrive} class="h-4 w-4 rounded" />
                <img src={googleDriveLogo} alt="Google Drive" class="h-5 w-5" />
                <div>
                    <div class="font-medium">Google Drive</div>
                    <div class="text-muted-foreground text-sm">
                        Index your Drive documents, spreadsheets, and presentations
                    </div>
                </div>
            </label>

            <label
                class="hover:bg-muted/50 flex cursor-pointer items-center gap-3 rounded-lg border p-3">
                <input type="checkbox" bind:checked={connectGmail} class="h-4 w-4 rounded" />
                <img src={gmailLogo} alt="Gmail" class="h-5 w-5" />
                <div>
                    <div class="font-medium">Gmail</div>
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
                disabled={isSubmitting || (!connectDrive && !connectGmail)}
                class="cursor-pointer">
                {isSubmitting ? 'Connecting...' : 'Connect with Google'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
