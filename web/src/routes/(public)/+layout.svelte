<script lang="ts">
    import '../../app.css'
    import omniLogo from '$lib/images/icons/omni-logo.png'
</script>

<div class="from-background to-muted min-h-screen bg-gradient-to-br">
    <div class="flex min-h-screen items-center justify-center p-4">
        <div class="w-full max-w-md">
            <div class="mb-8 text-center">
                <img src={omniLogo} alt="Omni" class="mx-auto mb-2 h-10 w-auto" />
                <p class="text-muted-foreground mt-2">Enterprise Search Platform</p>
            </div>
            <slot />
        </div>
    </div>
</div>
