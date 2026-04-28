<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import WebConnectorSetup from '$lib/components/web-connector-setup.svelte'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    function handleSuccess() {
        open = false
        if (onSuccess) {
            onSuccess()
        }
    }

    function handleCancel() {
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Web</Dialog.Title>
            <Dialog.Description>
                Configure your website crawler settings to index web content.
            </Dialog.Description>
        </Dialog.Header>

        <WebConnectorSetup onSuccess={handleSuccess} onCancel={handleCancel} />
    </Dialog.Content>
</Dialog.Root>
