<script lang="ts">
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import MaskedCredentialInput from '$lib/components/masked-credential-input.svelte'

    interface Props {
        serviceAccountJson?: string
        principalEmail?: string
        domain?: string
        hasStoredKey?: boolean
        disabled?: boolean
        // 'sa-direct' hides the admin-email field and shows
        // non-impersonation guidance. Defaults to DWD behavior.
        mode?: 'dwd' | 'sa-direct'
        // SA-direct group membership sync needs the Workspace domain but never
        // impersonates an admin user.
        showDomain?: boolean
        // Hide the SA-direct explanation paragraph when the surrounding page
        // already explains the mode (e.g. a banner on the settings page).
        showSaDirectInfo?: boolean
        onCredentialsChange?: () => void
        onAccountDetailsChange?: () => void
        onCredentialReplacementStart?: () => void
        onCredentialReplacementCancel?: () => void
    }

    let {
        serviceAccountJson = $bindable(''),
        principalEmail = $bindable(''),
        domain = $bindable(''),
        hasStoredKey = false,
        disabled = false,
        mode = 'dwd',
        showDomain = false,
        showSaDirectInfo = true,
        onCredentialsChange,
        onAccountDetailsChange,
        onCredentialReplacementStart,
        onCredentialReplacementCancel,
    }: Props = $props()

    const isSaDirect = $derived(mode === 'sa-direct')
</script>

<div class="space-y-4">
    <div class="space-y-2">
        <Label for="service-account-json">Service Account JSON Key</Label>
        <MaskedCredentialInput
            id="service-account-json"
            bind:value={serviceAccountJson}
            hasStoredValue={hasStoredKey}
            multiline
            {disabled}
            onValueChange={onCredentialsChange}
            onReplacementStart={onCredentialReplacementStart}
            onReplacementCancel={onCredentialReplacementCancel}
            placeholder="Paste your Google service account JSON key here..." />
        <p class="text-muted-foreground text-sm">
            Download this from the Google Cloud Console under "Service Accounts" > "Keys".
        </p>
    </div>

    {#if isSaDirect}
        {#if showSaDirectInfo}
            <p class="text-muted-foreground text-sm">
                The service account will authenticate as itself without domain-wide delegation. Add
                the service account email to each shared drive as
                <span class="font-medium">Content manager</span> or
                <span class="font-medium">Manager</span>.
            </p>
        {/if}
        {#if showDomain}
            <div class="space-y-2">
                <Label for="sa-direct-domain">Organization Domain</Label>
                <Input
                    id="sa-direct-domain"
                    name="domain"
                    bind:value={domain}
                    placeholder="yourdomain.com"
                    type="text"
                    {disabled}
                    oninput={onAccountDetailsChange ?? onCredentialsChange}
                    required />
                <p class="text-muted-foreground text-sm">
                    Used to validate and sync Google Workspace group memberships. This does not
                    impersonate a Workspace user.
                </p>
            </div>
        {/if}
    {:else}
        <div class="space-y-2">
            <Label for="principal-email">Admin Email</Label>
            <Input
                id="principal-email"
                name="principalEmail"
                bind:value={principalEmail}
                placeholder="admin@yourdomain.com"
                type="email"
                {disabled}
                oninput={onAccountDetailsChange ?? onCredentialsChange}
                required />
            <p class="text-muted-foreground text-sm">
                The admin user the service account impersonates to access Google Workspace APIs.
            </p>
        </div>

        <div class="space-y-2">
            <Label for="domain">Organization Domain</Label>
            <Input
                id="domain"
                name="domain"
                bind:value={domain}
                placeholder="yourdomain.com"
                type="text"
                {disabled}
                oninput={onAccountDetailsChange ?? onCredentialsChange}
                required />
            <p class="text-muted-foreground text-sm">
                Your Google Workspace domain (e.g., company.com).
            </p>
        </div>
    {/if}
</div>
