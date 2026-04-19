<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import { toast } from 'svelte-sonner'
    import type { FilesystemSourceConfig } from '$lib/types'
    import FilesystemConnectorForm from '$lib/components/filesystem-connector-form.svelte'

    interface Props {
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { onSuccess, onCancel }: Props = $props()

    let name = $state('')
    let basePath = $state('')
    let fileExtensions = $state<string[]>([])
    let excludePatterns = $state<string[]>([])
    let maxFileSizeMb = $state(10)
    let isSubmitting = $state(false)

    function validatePath(path: string): boolean {
        return path.startsWith('/')
    }

    async function handleSubmit() {
        if (!name.trim()) {
            toast.error('Name is required')
            return
        }

        if (!basePath.trim()) {
            toast.error('Base path is required')
            return
        }

        if (!validatePath(basePath)) {
            toast.error('Base path must be an absolute path (starting with /)')
            return
        }

        if (maxFileSizeMb < 1) {
            toast.error('Max file size must be at least 1 MB')
            return
        }

        isSubmitting = true
        try {
            const config: FilesystemSourceConfig = {
                base_path: basePath.trim(),
                file_extensions: fileExtensions.length > 0 ? fileExtensions : undefined,
                exclude_patterns: excludePatterns.length > 0 ? excludePatterns : undefined,
                max_file_size_bytes: maxFileSizeMb * 1024 * 1024,
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: name.trim(),
                    sourceType: 'local_files',
                    config: config,
                }),
            })

            if (!sourceResponse.ok) {
                const error = await sourceResponse.text()
                throw new Error(error || 'Failed to create filesystem source')
            }

            const source = await sourceResponse.json()

            // Trigger initial sync
            const syncResponse = await fetch(`/api/sources/${source.id}/sync`, {
                method: 'POST',
            })

            if (!syncResponse.ok) {
                console.warn('Failed to start initial sync, but source was created')
            }

            toast.success('Filesystem source connected successfully!')
            onSuccess?.()
        } catch (error: any) {
            console.error('Error setting up filesystem connector:', error)
            toast.error(error.message || 'Failed to set up filesystem connector')
        } finally {
            isSubmitting = false
        }
    }
</script>

<div class="space-y-4">
    <FilesystemConnectorForm
        bind:name
        bind:basePath
        bind:fileExtensions
        bind:excludePatterns
        bind:maxFileSizeMb
        disabled={isSubmitting} />

    <!-- Actions -->
    <div class="flex justify-end gap-2 pt-4">
        <Button
            variant="outline"
            onclick={() => onCancel?.()}
            disabled={isSubmitting}
            class="cursor-pointer">
            Cancel
        </Button>
        <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
            {isSubmitting ? 'Connecting...' : 'Connect'}
        </Button>
    </div>
</div>
