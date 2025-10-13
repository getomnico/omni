<script lang="ts">
    import '../../app.css'
    import omniLogo from '$lib/images/icons/omni-logo.png'
</script>

<div class="from-background to-muted min-h-screen bg-gradient-to-br">
    <div class="flex min-h-screen items-center justify-center p-4">
        <div class="w-full max-w-md">
            <div class="mb-8 text-center">
                <p class="text-2xl font-semibold">omni</p>
                <p class="text-muted-foreground mt-2">Your AI assistant for work</p>
            </div>
            <slot />
        </div>
    </div>
</div>
