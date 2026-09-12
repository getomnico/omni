<script lang="ts">
    import { goto } from '$app/navigation'
    import { resolve } from '$app/paths'
    import { z } from 'zod'
    import { toast } from 'svelte-sonner'
    import UserInput, { type ModelOption } from '$lib/components/user-input.svelte'
    import UploadChip from '$lib/components/upload-chip.svelte'
    import { resolvePreferredModelId, setPreferredModelId } from '$lib/preferences'
    import type { MentionedDocument } from '$lib/types/message'

    let {
        projectId,
        models,
        disabled = false,
    }: {
        projectId: string
        models: ModelOption[]
        disabled?: boolean
    } = $props()

    const chatResponse = z.object({ chatId: z.string().min(1) })
    const uploadResponse = z.object({ id: z.string().min(1), filename: z.string().min(1) })
    const messageResponse = z.object({ messageId: z.string().min(1) })
    type PendingUpload = { id: string; filename: string; uploading: boolean }

    let value = $state('')
    let mentionedDocs = $state<MentionedDocument[]>([])
    let uploads = $state<PendingUpload[]>([])
    let uploadInput = $state<HTMLInputElement>()
    let submitting = $state(false)
    let createdChatId = $state<string | null>(null)
    let messageSent = $state(false)
    let selectedModelId = $state<string | null>(resolvePreferredModelId(models))
    const canSubmit = $derived(
        !disabled &&
            !submitting &&
            !uploads.some((u) => u.uploading) &&
            (value.trim().length > 0 || mentionedDocs.length > 0 || uploads.length > 0),
    )

    async function attachFiles(files: FileList | null) {
        if (!files || disabled || submitting || messageSent) return
        for (const file of Array.from(files)) {
            const placeholderId = crypto.randomUUID()
            uploads.push({ id: placeholderId, filename: file.name, uploading: true })
            try {
                const body = new FormData()
                body.append('file', file)
                const response = await fetch('/api/uploads', { method: 'POST', body })
                if (!response.ok) throw new Error('Upload failed')
                const uploaded = uploadResponse.parse(await response.json())
                uploads = uploads.map((u) =>
                    u.id === placeholderId ? { ...uploaded, uploading: false } : u,
                )
            } catch {
                uploads = uploads.filter((u) => u.id !== placeholderId)
                toast.error(`Failed to upload ${file.name}`)
            }
        }
        if (uploadInput) uploadInput.value = ''
    }

    async function submit() {
        if (!canSubmit) return
        submitting = true
        try {
            // Reuse the chat if sending its first message failed, rather than creating duplicates.
            if (!createdChatId) {
                const response = await fetch('/api/chat', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ projectId, modelId: selectedModelId }),
                })
                if (!response.ok) throw new Error('Failed to create chat. Please try again.')
                createdChatId = chatResponse.parse(await response.json()).chatId
            }
            if (!messageSent) {
                const response = await fetch(`/api/chat/${createdChatId}/messages`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        content: value.trim(),
                        attachmentIds: uploads.map((u) => u.id),
                        mentionedDocuments: mentionedDocs,
                    }),
                })
                if (!response.ok)
                    throw new Error(
                        'Failed to send message. Your draft is saved here; please try again.',
                    )
                messageResponse.parse(await response.json())
                messageSent = true
            }
            await goto(resolve(`/chat/${createdChatId}`), {
                invalidateAll: true,
                state: { stream: true },
            })
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Failed to start chat')
        } finally {
            submitting = false
        }
    }
</script>

<input
    bind:this={uploadInput}
    type="file"
    multiple
    class="hidden"
    aria-label="Attach files"
    onchange={(event) => attachFiles(event.currentTarget.files)} />

{#snippet attachments()}
    {#if uploads.length > 0}
        <div class="flex flex-wrap gap-2">
            {#each uploads as upload (upload.id)}
                <UploadChip
                    filename={upload.filename}
                    uploading={upload.uploading}
                    onRemove={() => {
                        if (!submitting && !messageSent)
                            uploads = uploads.filter((u) => u.id !== upload.id)
                    }} />
            {/each}
        </div>
    {/if}
{/snippet}

<div inert={disabled || submitting || messageSent} class:opacity-60={disabled}>
    <UserInput
        bind:value
        bind:mentionedDocs
        inputMode="chat"
        modeSelectorEnabled={false}
        onSubmit={submit}
        onInput={(input) => (value = input)}
        onAttachClick={() => uploadInput?.click()}
        onFilesDropped={attachFiles}
        {attachments}
        {models}
        {selectedModelId}
        {canSubmit}
        disabled={disabled || submitting || messageSent}
        isLoading={submitting}
        placeholders={{ chat: 'Ask anything about this project...', search: 'Search...' }}
        maxWidth="max-w-none"
        onModelChange={(id) => {
            selectedModelId = id
            setPreferredModelId(id)
        }} />
</div>
{#if messageSent && !submitting && createdChatId}
    <a href={resolve(`/chat/${createdChatId}`)} class="mt-2 inline-block text-sm underline"
        >Open your chat</a>
{/if}
