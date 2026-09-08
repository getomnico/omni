<script lang="ts">
    import {
        File,
        FileSpreadsheet,
        FileText,
        FileType,
        Globe,
        Image as ImageIcon,
    } from '@lucide/svelte'
    import type { Component } from 'svelte'
    import type { ArtifactKind } from '$lib/utils/artifacts'
    import { cn } from '$lib/utils'

    type Props = {
        kind: ArtifactKind
        className?: string
    }

    const KindIcons: Record<ArtifactKind, Component> = {
        image: ImageIcon,
        pdf: FileText,
        html: Globe,
        docx: FileType,
        xlsx: FileSpreadsheet,
        text: FileText,
        other: File,
    }

    const KindColors: Record<ArtifactKind, string> = {
        image: 'text-violet-600',
        pdf: 'text-rose-600',
        html: 'text-orange-500',
        docx: 'text-blue-600',
        xlsx: 'text-emerald-600',
        text: 'text-muted-foreground',
        other: 'text-muted-foreground',
    }

    let { kind, className = 'h-4 w-4' }: Props = $props()

    const Icon = $derived(KindIcons[kind])
</script>

<Icon class={cn(KindColors[kind], className)} aria-hidden="true" />
