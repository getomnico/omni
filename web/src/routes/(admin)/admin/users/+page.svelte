<script lang="ts">
    import type { PageData } from './$types'
    import { enhance } from '$app/forms'

    export let data: PageData

    function formatDate(date: Date | null) {
        if (!date) return 'N/A'
        return new Date(date).toLocaleDateString()
    }
</script>

<div class="mx-auto max-w-screen-xl pt-8">
    <div>
        <h1 class="text-2xl font-bold tracking-tight">User Management</h1>
        <p class="text-muted-foreground">
            Create accounts for users in your org to give them access to Omni.
        </p>
    </div>

    <div class="mx-auto max-w-7xl py-4">
        <div class="">
            <div class="ring-border mt-2 overflow-hidden shadow ring-1 md:rounded-lg">
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
                            <th class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                >Updated</th>
                            <th class="relative px-6 py-3"><span class="sr-only">Actions</span></th>
                        </tr>
                    </thead>
                    <tbody class="divide-border bg-card divide-y">
                        {#each data.users as user}
                            <tr>
                                <td class="text-foreground px-6 py-4 text-sm font-medium">
                                    {user.email}
                                </td>
                                <td class="text-muted-foreground px-6 py-4 text-sm">
                                    <form
                                        method="POST"
                                        action="?/updateRole"
                                        use:enhance
                                        class="inline">
                                        <input type="hidden" name="userId" value={user.id} />
                                        <select
                                            name="role"
                                            value={user.role}
                                            on:change={(e) => e.currentTarget.form?.requestSubmit()}
                                            class="border-border bg-background rounded-md text-sm">
                                            <option value="admin">Admin</option>
                                            <option value="user">User</option>
                                            <option value="viewer">Viewer</option>
                                        </select>
                                    </form>
                                </td>
                                <td class="px-6 py-4 text-sm">
                                    <span
                                        class="inline-flex rounded-full px-2 text-xs leading-5 font-semibold
											{user.isActive
                                            ? 'bg-green-100 text-green-800 dark:bg-green-900/20 dark:text-green-400'
                                            : 'bg-destructive/10 text-destructive'}
										">
                                        {user.isActive ? 'Active' : 'Inactive'}
                                    </span>
                                </td>
                                <td class="text-muted-foreground px-6 py-4 text-sm">
                                    {formatDate(user.createdAt)}
                                </td>
                                <td class="text-muted-foreground px-6 py-4 text-sm">
                                    {formatDate(user.updatedAt)}
                                </td>
                                <td class="px-6 py-4 text-right text-sm font-medium">
                                    <div class="flex justify-end space-x-2">
                                        {#if user.isActive}
                                            <form method="POST" action="?/suspend" use:enhance>
                                                <input
                                                    type="hidden"
                                                    name="userId"
                                                    value={user.id} />
                                                <button
                                                    type="submit"
                                                    class="text-destructive hover:text-destructive/80">
                                                    Deactivate
                                                </button>
                                            </form>
                                        {:else}
                                            <form method="POST" action="?/activate" use:enhance>
                                                <input
                                                    type="hidden"
                                                    name="userId"
                                                    value={user.id} />
                                                <button
                                                    type="submit"
                                                    class="text-green-600 hover:text-green-900 dark:text-green-400 dark:hover:text-green-300">
                                                    Activate
                                                </button>
                                            </form>
                                        {/if}
                                    </div>
                                </td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            </div>
        </div>
    </div>
</div>
