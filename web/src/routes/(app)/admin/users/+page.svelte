<script lang="ts">
	import type { PageData } from './$types';
	import { enhance } from '$app/forms';

	export let data: PageData;

	function formatDate(date: Date | null) {
		if (!date) return 'N/A';
		return new Date(date).toLocaleDateString();
	}
</script>

<div class="min-h-screen bg-gray-50">
	<nav class="bg-white shadow">
		<div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
			<div class="flex h-16 justify-between">
				<div class="flex items-center">
					<h1 class="text-xl font-semibold">Clio Admin - User Management</h1>
				</div>
				<div class="flex items-center space-x-4">
					<a href="/" class="text-sm text-gray-500 hover:text-gray-700">Back to Home</a>
				</div>
			</div>
		</div>
	</nav>

	<main class="mx-auto max-w-7xl px-4 py-8 sm:px-6 lg:px-8">
		<div class="rounded-lg bg-white shadow">
			<div class="px-4 py-5 sm:p-6">
				<h2 class="text-lg font-medium text-gray-900">Users</h2>
				
				<div class="mt-6 overflow-hidden shadow ring-1 ring-black ring-opacity-5 md:rounded-lg">
					<table class="min-w-full divide-y divide-gray-300">
						<thead class="bg-gray-50">
							<tr>
								<th class="px-6 py-3 text-left text-sm font-semibold text-gray-900">Username</th>
								<th class="px-6 py-3 text-left text-sm font-semibold text-gray-900">Email</th>
								<th class="px-6 py-3 text-left text-sm font-semibold text-gray-900">Role</th>
								<th class="px-6 py-3 text-left text-sm font-semibold text-gray-900">Status</th>
								<th class="px-6 py-3 text-left text-sm font-semibold text-gray-900">Created</th>
								<th class="px-6 py-3 text-left text-sm font-semibold text-gray-900">Approved</th>
								<th class="relative px-6 py-3"><span class="sr-only">Actions</span></th>
							</tr>
						</thead>
						<tbody class="divide-y divide-gray-200 bg-white">
							{#each data.users as user}
								<tr>
									<td class="px-6 py-4 text-sm font-medium text-gray-900">
										{user.username}
									</td>
									<td class="px-6 py-4 text-sm text-gray-500">
										{user.email}
									</td>
									<td class="px-6 py-4 text-sm text-gray-500">
										<form method="POST" action="?/updateRole" use:enhance class="inline">
											<input type="hidden" name="userId" value={user.id} />
											<select
												name="role"
												value={user.role}
												on:change={(e) => e.currentTarget.form?.requestSubmit()}
												class="rounded-md border-gray-300 text-sm"
											>
												<option value="admin">Admin</option>
												<option value="user">User</option>
												<option value="viewer">Viewer</option>
											</select>
										</form>
									</td>
									<td class="px-6 py-4 text-sm">
										<span class="inline-flex rounded-full px-2 text-xs font-semibold leading-5
											{user.status === 'active' ? 'bg-green-100 text-green-800' : ''}
											{user.status === 'pending' ? 'bg-yellow-100 text-yellow-800' : ''}
											{user.status === 'suspended' ? 'bg-red-100 text-red-800' : ''}
										">
											{user.status}
										</span>
									</td>
									<td class="px-6 py-4 text-sm text-gray-500">
										{formatDate(user.createdAt)}
									</td>
									<td class="px-6 py-4 text-sm text-gray-500">
										{formatDate(user.approvedAt)}
									</td>
									<td class="px-6 py-4 text-right text-sm font-medium">
										<div class="flex justify-end space-x-2">
											{#if user.status === 'pending'}
												<form method="POST" action="?/approve" use:enhance>
													<input type="hidden" name="userId" value={user.id} />
													<button
														type="submit"
														class="text-indigo-600 hover:text-indigo-900"
													>
														Approve
													</button>
												</form>
											{/if}
											{#if user.status === 'active'}
												<form method="POST" action="?/suspend" use:enhance>
													<input type="hidden" name="userId" value={user.id} />
													<button
														type="submit"
														class="text-red-600 hover:text-red-900"
													>
														Suspend
													</button>
												</form>
											{/if}
											{#if user.status === 'suspended'}
												<form method="POST" action="?/activate" use:enhance>
													<input type="hidden" name="userId" value={user.id} />
													<button
														type="submit"
														class="text-green-600 hover:text-green-900"
													>
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
	</main>
</div>