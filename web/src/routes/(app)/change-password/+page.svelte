<script lang="ts">
    import { enhance } from '$app/forms'
    import { goto } from '$app/navigation'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import {
        Card,
        CardContent,
        CardDescription,
        CardHeader,
        CardTitle,
    } from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { toast } from 'svelte-sonner'
    import type { PageData, ActionData } from './$types'

    let { data, form }: { data: PageData; form: ActionData } = $props()

    let isSubmitting = $state(false)
    let currentPassword = $state('')
    let newPassword = $state('')
    let confirmPassword = $state('')
    let showPassword = $state(false)

    function getPasswordStrength(password: string): {
        strength: number
        label: string
        color: string
    } {
        if (!password) return { strength: 0, label: '', color: '' }

        let strength = 0
        if (password.length >= 8) strength += 1
        if (password.length >= 12) strength += 1
        if (/[a-z]/.test(password) && /[A-Z]/.test(password)) strength += 1
        if (/\d/.test(password)) strength += 1
        if (/[^a-zA-Z0-9]/.test(password)) strength += 1

        if (strength <= 2) return { strength, label: 'Weak', color: 'text-red-600' }
        if (strength <= 3) return { strength, label: 'Medium', color: 'text-yellow-600' }
        return { strength, label: 'Strong', color: 'text-green-600' }
    }

    let passwordStrength = $derived(getPasswordStrength(newPassword))
</script>

<div class="flex min-h-screen items-center justify-center p-4">
    <Card class="w-full max-w-md">
        <CardHeader>
            <CardTitle>Change Password</CardTitle>
            <CardDescription>
                {#if data.mustChangePassword}
                    You must change your password before continuing.
                {:else}
                    Update your account password
                {/if}
            </CardDescription>
        </CardHeader>
        <CardContent>
            {#if data.mustChangePassword}
                <Alert.Root class="mb-4">
                    <Alert.Description>
                        Your account has a temporary password. Please set a new password to
                        continue.
                    </Alert.Description>
                </Alert.Root>
            {/if}

            <form
                method="POST"
                use:enhance={() => {
                    isSubmitting = true
                    return async ({ result, update }) => {
                        if (result.type === 'success') {
                            toast.success('Password changed successfully')
                            goto('/')
                            return
                        }
                        await update()
                        isSubmitting = false
                    }
                }}>
                <div class="space-y-4">
                    <div class="space-y-2">
                        <Label for="currentPassword">Current Password</Label>
                        <Input
                            id="currentPassword"
                            name="currentPassword"
                            type={showPassword ? 'text' : 'password'}
                            required
                            bind:value={currentPassword}
                            class={form?.field === 'currentPassword' ? 'border-destructive' : ''} />
                        {#if form?.field === 'currentPassword' && form?.error}
                            <p class="text-destructive text-sm">{form.error}</p>
                        {/if}
                    </div>

                    <div class="space-y-2">
                        <Label for="newPassword">New Password</Label>
                        <Input
                            id="newPassword"
                            name="newPassword"
                            type={showPassword ? 'text' : 'password'}
                            required
                            bind:value={newPassword}
                            class={form?.field === 'newPassword' ? 'border-destructive' : ''} />
                        {#if newPassword}
                            <div class="flex items-center gap-2 text-sm">
                                <div class="bg-muted h-2 flex-1 overflow-hidden rounded-full">
                                    <div
                                        class="bg-primary h-full transition-all"
                                        style="width: {(passwordStrength.strength / 5) * 100}%">
                                    </div>
                                </div>
                                <span class={passwordStrength.color}>
                                    {passwordStrength.label}
                                </span>
                            </div>
                        {/if}
                        {#if form?.field === 'newPassword' && form?.error}
                            <p class="text-destructive text-sm">{form.error}</p>
                        {/if}
                        <p class="text-muted-foreground text-xs">
                            Must be at least 8 characters long
                        </p>
                    </div>

                    <div class="space-y-2">
                        <Label for="confirmPassword">Confirm New Password</Label>
                        <Input
                            id="confirmPassword"
                            name="confirmPassword"
                            type={showPassword ? 'text' : 'password'}
                            required
                            bind:value={confirmPassword}
                            class={form?.field === 'confirmPassword' ? 'border-destructive' : ''} />
                        {#if form?.field === 'confirmPassword' && form?.error}
                            <p class="text-destructive text-sm">{form.error}</p>
                        {/if}
                    </div>

                    <div class="flex items-center space-x-2">
                        <input
                            type="checkbox"
                            id="showPassword"
                            bind:checked={showPassword}
                            class="border-input bg-background ring-offset-background h-4 w-4 rounded" />
                        <Label for="showPassword" class="text-sm font-normal">Show passwords</Label>
                    </div>

                    {#if form?.field === 'general' && form?.error}
                        <Alert.Root variant="destructive">
                            <Alert.Description>{form.error}</Alert.Description>
                        </Alert.Root>
                    {/if}

                    <div class="flex gap-2 pt-4">
                        {#if !data.mustChangePassword}
                            <Button
                                type="button"
                                variant="outline"
                                onclick={() => goto('/')}
                                class="flex-1">
                                Cancel
                            </Button>
                        {/if}
                        <Button
                            type="submit"
                            disabled={isSubmitting}
                            class={data.mustChangePassword ? 'w-full' : 'flex-1'}>
                            {isSubmitting ? 'Changing Password...' : 'Change Password'}
                        </Button>
                    </div>
                </div>
            </form>
        </CardContent>
    </Card>
</div>
