<script lang="ts">
    import { enhance } from '$app/forms'
    import { invalidateAll } from '$app/navigation'
    import * as Dialog from '$lib/components/ui/dialog'
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import * as Tooltip from '$lib/components/ui/tooltip'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Badge } from '$lib/components/ui/badge'
    import { toast } from 'svelte-sonner'
    import type { PageData, ActionData } from './$types'
    import { Plus, Pencil, Key, Power, Trash2, Search } from '@lucide/svelte'

    let { data, form }: { data: PageData; form: ActionData } = $props()

    let showCreateDialog = $state(false)
    let showEditDialog = $state(false)
    let showDeleteDialog = $state(false)
    let showPasswordDialog = $state(false)
    let isSubmitting = $state(false)
    let searchQuery = $state(data.searchQuery || '')

    let generatedPassword = $state('')
    let passwordUserEmail = $state('')

    let createForm = $state({
        email: '',
        role: 'user' as 'admin' | 'user' | 'viewer',
    })

    let editForm = $state({
        userId: '',
        email: '',
        role: 'user' as 'admin' | 'user' | 'viewer',
    })

    let deleteUserId = $state('')
    let deleteUserEmail = $state('')

    function formatDate(date: Date | null) {
        if (!date) return 'N/A'
        return new Date(date).toLocaleDateString()
    }

    function openCreateDialog() {
        createForm = {
            email: '',
            role: 'user',
        }
        showCreateDialog = true
    }

    function openEditDialog(user: any) {
        editForm = {
            userId: user.id,
            email: user.email,
            role: user.role,
        }
        showEditDialog = true
    }

    function openDeleteDialog(user: any) {
        deleteUserId = user.id
        deleteUserEmail = user.email
        showDeleteDialog = true
    }

    async function copyPassword() {
        try {
            await navigator.clipboard.writeText(generatedPassword)
            toast.success('Password copied to clipboard')
        } catch (err) {
            toast.error('Failed to copy password')
        }
    }

    $effect(() => {
        if (form?.success) {
            isSubmitting = false
            if (form.action === 'createUser' && form.password) {
                showCreateDialog = false
                generatedPassword = form.password
                passwordUserEmail = form.email || ''
                showPasswordDialog = true
                toast.success('User created successfully')
            } else if (form.action === 'resetPassword' && form.password) {
                generatedPassword = form.password
                passwordUserEmail = form.email || ''
                showPasswordDialog = true
                toast.success('Password reset successfully')
            } else if (form.action === 'updateUser') {
                showEditDialog = false
                toast.success('User updated successfully')
            } else if (form.action === 'deleteUser') {
                showDeleteDialog = false
                toast.success('User deleted successfully')
            } else if (form.action === 'toggleActive') {
                toast.success(form.message || 'User status updated')
            }
            invalidateAll()
        } else if (form?.error) {
            isSubmitting = false
            toast.error(form.error)
        }
    })
</script>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div class="flex items-center justify-between">
            <div>
                <h1 class="text-3xl font-bold tracking-tight">User Management</h1>
                <p class="text-muted-foreground mt-2">
                    Create and manage user accounts and permissions
                </p>
            </div>
            <Button onclick={openCreateDialog} class="cursor-pointer">
                <Plus />
                Add User
            </Button>
        </div>

        <div class="space-y-3">
            <form method="GET" class="relative max-w-md">
                <Search
                    class="text-muted-foreground pointer-events-none absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2" />
                <Input
                    type="search"
                    name="search"
                    placeholder="Search users by email..."
                    value={searchQuery}
                    class="bg-card pl-9" />
            </form>

            <div class="ring-border overflow-hidden rounded-lg shadow ring-1">
                <table class="divide-border min-w-full divide-y">
                    <thead class="bg-muted/50">
                        <tr>
                            <th class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                >Email</th>
                            <th class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                >Role</th>
                            <th class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                >Status</th>
                            <th class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                >Created</th>
                            <th class="text-foreground px-6 py-3 text-right text-sm font-semibold"
                            ></th>
                        </tr>
                    </thead>
                    <tbody class="divide-border bg-card divide-y">
                        {#each data.users as user}
                            <tr class="group">
                                <td class="px-6 py-4">
                                    <div class="text-foreground text-sm font-medium">
                                        {user.email}
                                    </div>
                                    {#if user.mustChangePassword}
                                        <Badge variant="outline" class="mt-1 text-xs"
                                            >Must change password</Badge>
                                    {/if}
                                </td>
                                <td class="text-muted-foreground px-6 py-4 text-sm capitalize">
                                    {user.role}
                                </td>
                                <td class="px-6 py-4 text-sm">
                                    <Badge
                                        variant={user.isActive ? 'secondary' : 'destructive'}
                                        class={user.isActive
                                            ? 'border-green-200 bg-green-100 text-green-800 dark:border-green-800 dark:bg-green-900/20 dark:text-green-400'
                                            : ''}>
                                        {user.isActive ? 'Active' : 'Inactive'}
                                    </Badge>
                                </td>
                                <td class="text-muted-foreground px-6 py-4 text-sm">
                                    {formatDate(user.createdAt)}
                                </td>
                                <td class="px-6 py-4 text-right text-sm">
                                    <div
                                        class="flex justify-end gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                                        <Tooltip.Provider delayDuration={300}>
                                            <Tooltip.Root>
                                                <Tooltip.Trigger>
                                                    <Button
                                                        variant="ghost"
                                                        size="icon"
                                                        class="h-8 w-8 cursor-pointer"
                                                        onclick={() => openEditDialog(user)}>
                                                        <Pencil class="h-4 w-4" />
                                                    </Button>
                                                </Tooltip.Trigger>
                                                <Tooltip.Content>
                                                    <p>Edit user</p>
                                                </Tooltip.Content>
                                            </Tooltip.Root>
                                        </Tooltip.Provider>

                                        <Tooltip.Provider delayDuration={300}>
                                            <Tooltip.Root>
                                                <Tooltip.Trigger>
                                                    <form
                                                        method="POST"
                                                        action="?/resetPassword"
                                                        use:enhance={() => {
                                                            isSubmitting = true
                                                            return async ({ update }) => {
                                                                await update()
                                                            }
                                                        }}>
                                                        <input
                                                            type="hidden"
                                                            name="userId"
                                                            value={user.id} />
                                                        <Button
                                                            type="submit"
                                                            variant="ghost"
                                                            size="icon"
                                                            class="h-8 w-8 cursor-pointer"
                                                            disabled={isSubmitting}>
                                                            <Key class="h-4 w-4" />
                                                        </Button>
                                                    </form>
                                                </Tooltip.Trigger>
                                                <Tooltip.Content>
                                                    <p>Reset password</p>
                                                </Tooltip.Content>
                                            </Tooltip.Root>
                                        </Tooltip.Provider>

                                        <Tooltip.Provider delayDuration={300}>
                                            <Tooltip.Root>
                                                <Tooltip.Trigger>
                                                    <form
                                                        method="POST"
                                                        action="?/toggleActive"
                                                        use:enhance={() => {
                                                            isSubmitting = true
                                                            return async ({ update }) => {
                                                                await update()
                                                            }
                                                        }}>
                                                        <input
                                                            type="hidden"
                                                            name="userId"
                                                            value={user.id} />
                                                        <Button
                                                            type="submit"
                                                            variant="ghost"
                                                            size="icon"
                                                            class="h-8 w-8 cursor-pointer"
                                                            disabled={isSubmitting}>
                                                            <Power class="h-4 w-4" />
                                                        </Button>
                                                    </form>
                                                </Tooltip.Trigger>
                                                <Tooltip.Content>
                                                    <p>
                                                        {user.isActive ? 'Deactivate' : 'Activate'} user
                                                    </p>
                                                </Tooltip.Content>
                                            </Tooltip.Root>
                                        </Tooltip.Provider>

                                        <Tooltip.Provider delayDuration={300}>
                                            <Tooltip.Root>
                                                <Tooltip.Trigger>
                                                    <Button
                                                        variant="ghost"
                                                        size="icon"
                                                        class="text-destructive hover:text-destructive h-8 w-8 cursor-pointer"
                                                        onclick={() => openDeleteDialog(user)}>
                                                        <Trash2 class="h-4 w-4" />
                                                    </Button>
                                                </Tooltip.Trigger>
                                                <Tooltip.Content>
                                                    <p>Delete user</p>
                                                </Tooltip.Content>
                                            </Tooltip.Root>
                                        </Tooltip.Provider>
                                    </div>
                                </td>
                            </tr>
                        {/each}
                        {#if data.users.length === 0}
                            <tr>
                                <td colspan="5" class="text-muted-foreground px-6 py-8 text-center">
                                    No users found
                                </td>
                            </tr>
                        {/if}
                    </tbody>
                </table>
            </div>
        </div>
    </div>
</div>

<Dialog.Root bind:open={showCreateDialog}>
    <Dialog.Content>
        <Dialog.Header>
            <Dialog.Title>Create New User</Dialog.Title>
            <Dialog.Description>
                Create a new user account. A temporary password will be generated and must be
                changed on first login.
            </Dialog.Description>
        </Dialog.Header>
        <form
            method="POST"
            action="?/createUser"
            use:enhance={() => {
                isSubmitting = true
                return async ({ update }) => {
                    await update()
                }
            }}>
            <div class="space-y-4 py-4">
                <div class="space-y-2">
                    <Label for="email">Email</Label>
                    <Input
                        id="email"
                        name="email"
                        type="email"
                        required
                        bind:value={createForm.email}
                        placeholder="user@example.com" />
                </div>
                <div class="space-y-2">
                    <Label for="role">Role</Label>
                    <select
                        id="role"
                        name="role"
                        bind:value={createForm.role}
                        class="border-input bg-background ring-offset-background placeholder:text-muted-foreground focus-visible:ring-ring flex h-10 w-full rounded-md border px-3 py-2 text-sm focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50">
                        <option value="viewer">Viewer</option>
                        <option value="user">User</option>
                        <option value="admin">Admin</option>
                    </select>
                </div>
            </div>
            <Dialog.Footer>
                <Button type="button" variant="outline" onclick={() => (showCreateDialog = false)}>
                    Cancel
                </Button>
                <Button type="submit" disabled={isSubmitting}>
                    {isSubmitting ? 'Creating...' : 'Create User'}
                </Button>
            </Dialog.Footer>
        </form>
    </Dialog.Content>
</Dialog.Root>

<Dialog.Root bind:open={showEditDialog}>
    <Dialog.Content>
        <Dialog.Header>
            <Dialog.Title>Edit User</Dialog.Title>
            <Dialog.Description>Update user information and role</Dialog.Description>
        </Dialog.Header>
        <form
            method="POST"
            action="?/updateUser"
            use:enhance={() => {
                isSubmitting = true
                return async ({ update }) => {
                    await update()
                }
            }}>
            <input type="hidden" name="userId" value={editForm.userId} />
            <div class="space-y-4 py-4">
                <div class="space-y-2">
                    <Label for="edit-email">Email</Label>
                    <Input
                        id="edit-email"
                        name="email"
                        type="email"
                        required
                        bind:value={editForm.email} />
                </div>
                <div class="space-y-2">
                    <Label for="edit-role">Role</Label>
                    <select
                        id="edit-role"
                        name="role"
                        bind:value={editForm.role}
                        class="border-input bg-background ring-offset-background placeholder:text-muted-foreground focus-visible:ring-ring flex h-10 w-full rounded-md border px-3 py-2 text-sm focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50">
                        <option value="viewer">Viewer</option>
                        <option value="user">User</option>
                        <option value="admin">Admin</option>
                    </select>
                </div>
            </div>
            <Dialog.Footer>
                <Button type="button" variant="outline" onclick={() => (showEditDialog = false)}>
                    Cancel
                </Button>
                <Button type="submit" disabled={isSubmitting}>
                    {isSubmitting ? 'Updating...' : 'Update User'}
                </Button>
            </Dialog.Footer>
        </form>
    </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={showDeleteDialog}>
    <AlertDialog.Content>
        <AlertDialog.Header>
            <AlertDialog.Title>Are you sure?</AlertDialog.Title>
            <AlertDialog.Description>
                This will permanently delete the user account for <strong>{deleteUserEmail}</strong
                >. This action cannot be undone.
            </AlertDialog.Description>
        </AlertDialog.Header>
        <AlertDialog.Footer>
            <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
            <form
                method="POST"
                action="?/deleteUser"
                use:enhance={() => {
                    isSubmitting = true
                    return async ({ update }) => {
                        await update()
                    }
                }}>
                <input type="hidden" name="userId" value={deleteUserId} />
                <AlertDialog.Action type="submit" disabled={isSubmitting} class="bg-destructive">
                    {isSubmitting ? 'Deleting...' : 'Delete User'}
                </AlertDialog.Action>
            </form>
        </AlertDialog.Footer>
    </AlertDialog.Content>
</AlertDialog.Root>

<Dialog.Root bind:open={showPasswordDialog}>
    <Dialog.Content>
        <Dialog.Header>
            <Dialog.Title>Temporary Password</Dialog.Title>
            <Dialog.Description>
                Share this temporary password with <strong>{passwordUserEmail}</strong>. They will
                be required to change it on first login.
            </Dialog.Description>
        </Dialog.Header>
        <div class="space-y-4 py-4">
            <div class="space-y-2">
                <Label>Password</Label>
                <div class="bg-muted flex items-center gap-2 rounded-md p-3">
                    <code class="text-foreground flex-1 font-mono text-sm">
                        {generatedPassword}
                    </code>
                    <Button type="button" size="sm" variant="outline" onclick={copyPassword}>
                        Copy
                    </Button>
                </div>
            </div>
            <div class="bg-muted/50 rounded-md border p-3 text-sm">
                <p class="text-muted-foreground">
                    <strong>Important:</strong> Make sure to copy and share this password securely. It
                    will not be shown again.
                </p>
            </div>
        </div>
        <Dialog.Footer>
            <Button type="button" onclick={() => (showPasswordDialog = false)}>Done</Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
