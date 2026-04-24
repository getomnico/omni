<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import FilesystemConnectorSetup from '$lib/components/filesystem-connector-setup.svelte'

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
            <Dialog.Title>Connect Filesystem</Dialog.Title>
            <Dialog.Description>
                Configure a local directory to index its files and make them searchable.
            </Dialog.Description>
        </Dialog.Header>

        <FilesystemConnectorSetup onSuccess={handleSuccess} onCancel={handleCancel} />
    </Dialog.Content>
</Dialog.Root>
