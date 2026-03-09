# Microsoft 365 Connector Setup Guide — Azure Cloud (Docker Compose on a Single VM)

This guide walks a brand-new customer — starting with **no Azure subscription** — through every step required to deploy Omni on an Azure VM and connect it to Microsoft 365 services (Outlook Mail, Outlook Calendar, OneDrive, and SharePoint).

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Create an Azure Account & Subscription](#2-create-an-azure-account--subscription)
3. [Install the Azure CLI](#3-install-the-azure-cli)
4. [Provision an Azure VM](#4-provision-an-azure-vm)
5. [Configure Networking (NSG Rules)](#5-configure-networking-nsg-rules)
6. [Set Up DNS (Optional but Recommended)](#6-set-up-dns-optional-but-recommended)
7. [Install Docker & Docker Compose on the VM](#7-install-docker--docker-compose-on-the-vm)
8. [Register an App in Microsoft Entra ID](#8-register-an-app-in-microsoft-entra-id)
9. [Configure Microsoft Graph API Permissions](#9-configure-microsoft-graph-api-permissions)
10. [Create a Client Secret](#10-create-a-client-secret)
11. [Grant Admin Consent](#11-grant-admin-consent)
12. [Deploy Omni via Docker Compose](#12-deploy-omni-via-docker-compose)
13. [Connect Microsoft 365 Sources in Omni](#13-connect-microsoft-365-sources-in-omni)
14. [Verify the Integration](#14-verify-the-integration)
15. [Security Hardening Checklist](#15-security-hardening-checklist)
16. [Troubleshooting](#16-troubleshooting)

---

## 1. Prerequisites

| Item | Details |
|------|---------|
| A Microsoft account (personal or work) | Used to create the Azure subscription |
| A valid credit/debit card | Required for identity verification, even on the free tier |
| A domain name (optional) | For TLS/HTTPS — you can start with the VM's public IP |
| Microsoft 365 tenant | The organization whose data you want to index. You need **Global Administrator** or **Application Administrator** role to grant admin consent |

---

## 2. Create an Azure Account & Subscription

If you don't have an Azure account yet:

1. Go to [https://azure.microsoft.com/en-us/pricing/purchase-options/azure-account](https://azure.microsoft.com/en-us/pricing/purchase-options/azure-account).
2. Click **Try Azure for free** or **Pay as you go**.
   - The **free account** gives you **$200 credit for 30 days** and 12 months of popular services at no cost.
3. Sign in with your Microsoft account (or create one).
4. Complete the identity verification (phone + credit card).
5. Once your subscription is active, note your **Subscription ID** — you'll need it later.

> **Tip:** For production workloads beyond the trial, switch to a **Pay-As-You-Go** or **Enterprise Agreement** subscription.

---

## 3. Install the Azure CLI

On your local machine (the machine you'll use to manage Azure):

**macOS:**
```bash
brew install azure-cli
```

**Linux (Ubuntu/Debian):**
```bash
curl -sL https://aka.ms/InstallAzureCLIDeb | sudo bash
```

**Windows:**
```bash
winget install Microsoft.AzureCLI
```

Then log in:

```bash
az login
```

This opens a browser window for authentication. After login, confirm your subscription:

```bash
az account show --query "{name:name, id:id, tenantId:tenantId}" -o table
```

Note the **tenantId** — you'll need this when configuring the Microsoft connector.

---

## 4. Provision an Azure VM

### Create a Resource Group

```bash
az group create \
  --name omni-rg \
  --location eastus
```

### Create the VM

We recommend **Standard_D4s_v5** (4 vCPUs, 16 GB RAM) as a minimum for running all Omni services. For larger deployments, use D8s_v5 or higher.

```bash
az vm create \
  --resource-group omni-rg \
  --name omni-vm \
  --image Ubuntu2404 \
  --size Standard_D4s_v5 \
  --admin-username azureuser \
  --generate-ssh-keys \
  --os-disk-size-gb 128 \
  --public-ip-sku Standard
```

Save the output — it contains the **publicIpAddress** you'll use to SSH in and access the Omni UI.

### SSH into the VM

```bash
ssh azureuser@<publicIpAddress>
```

---

## 5. Configure Networking (NSG Rules)

Azure creates a Network Security Group (NSG) automatically with your VM. By default it allows SSH (port 22). You need to add rules for HTTP/HTTPS access to the Omni UI.

### Open port 443 (HTTPS — recommended for production)

```bash
az network nsg rule create \
  --resource-group omni-rg \
  --nsg-name omni-vmNSG \
  --name AllowHTTPS \
  --priority 1001 \
  --destination-port-ranges 443 \
  --protocol Tcp \
  --access Allow \
  --direction Inbound
```

### Open port 80 (HTTP — for Let's Encrypt ACME challenges or dev testing)

```bash
az network nsg rule create \
  --resource-group omni-rg \
  --nsg-name omni-vmNSG \
  --name AllowHTTP \
  --priority 1002 \
  --destination-port-ranges 80 \
  --protocol Tcp \
  --access Allow \
  --direction Inbound
```

### Restrict SSH to your IP (strongly recommended)

```bash
az network nsg rule update \
  --resource-group omni-rg \
  --nsg-name omni-vmNSG \
  --name default-allow-ssh \
  --source-address-prefixes <YOUR_PUBLIC_IP>/32
```

> **Important:** Never expose internal service ports (3001–3004, 4001–4009, 5432, 6379) to the internet. Docker Compose's internal `omni-network` handles all inter-service communication.

---

## 6. Set Up DNS (Optional but Recommended)

For production with TLS:

1. In your DNS provider, create an **A record** pointing your domain (e.g., `omni.yourcompany.com`) to the VM's public IP.
2. Omni's built-in Caddy reverse proxy will automatically provision a TLS certificate via Let's Encrypt.

If you skip this step, you can access Omni via `http://<VM_PUBLIC_IP>:3000` for development/testing.

---

## 7. Install Docker & Docker Compose on the VM

SSH into the VM and run:

```bash
# Install Docker
curl -fsSL https://get.docker.com | sudo sh

# Add your user to the docker group (avoids needing sudo)
sudo usermod -aG docker $USER

# Log out and back in for group change to take effect
exit
```

SSH back in, then verify:

```bash
docker --version
docker compose version
```

Both commands should succeed. Docker Compose v2 is bundled with modern Docker installations.

---

## 8. Register an App in Microsoft Entra ID

This creates the identity that Omni uses to access Microsoft 365 data via the Graph API.

1. Go to the [Azure Portal](https://portal.azure.com).
2. Navigate to **Microsoft Entra ID** > **App registrations** > **New registration**.
3. Fill in:
   - **Name:** `Omni - Microsoft 365 Connector`
   - **Supported account types:** Select **Accounts in this organizational directory only** (single-tenant) — this is the most common and most secure choice.
   - **Redirect URI:** Leave blank (not needed for client credentials flow).
4. Click **Register**.
5. On the app's **Overview** page, copy and save:
   - **Application (client) ID** → this is your `client_id`
   - **Directory (tenant) ID** → this is your `tenant_id`

---

## 9. Configure Microsoft Graph API Permissions

The Omni Microsoft connector requires these **Application permissions** (not Delegated):

| Permission | Used For |
|------------|----------|
| `User.Read.All` | Enumerating users in the tenant to iterate their data |
| `Files.Read.All` | Reading OneDrive files and SharePoint document libraries |
| `Mail.Read` | Reading Outlook inbox messages |
| `Calendars.Read` | Reading Outlook calendar events |
| `Sites.Read.All` | Enumerating SharePoint sites and their document libraries |

### Steps:

1. In your app registration, go to **API permissions**.
2. Click **Add a permission** > **Microsoft Graph** > **Application permissions**.
3. Search for and add each of the five permissions listed above:
   - `User.Read.All`
   - `Files.Read.All`
   - `Mail.Read`
   - `Calendars.Read`
   - `Sites.Read.All`
4. Click **Add permissions** after selecting all five.

> **Note:** These are **Application permissions**, not Delegated. Application permissions allow Omni to access data for all users in the tenant without requiring each user to sign in. This is the "app-only" client credentials flow.

---

## 10. Create a Client Secret

1. In your app registration, go to **Certificates & secrets**.
2. Click **Client secrets** > **New client secret**.
3. Enter a description (e.g., `Omni connector`) and choose an expiration:
   - **Recommended:** 24 months for production (set a calendar reminder to rotate before expiry).
4. Click **Add**.
5. **Immediately copy the secret Value** (not the Secret ID) — you will not be able to see it again after leaving this page.
   - This is your `client_secret`.

---

## 11. Grant Admin Consent

Application permissions require a tenant administrator to grant consent:

1. In your app registration, go to **API permissions**.
2. Click **Grant admin consent for [Your Organization]**.
3. Click **Yes** to confirm.
4. All five permissions should now show a green checkmark under the **Status** column, reading **Granted for [Your Organization]**.

> **Important:** If you don't see the "Grant admin consent" button, you don't have sufficient privileges. You need to be a **Global Administrator** or **Application Administrator** in the Microsoft 365 tenant.

---

## 12. Deploy Omni via Docker Compose

### Clone the Omni Repository

On the VM:

```bash
git clone https://github.com/getomnico/omni.git
cd omni
```

### Configure Environment Variables

```bash
cp .env.example .env
```

Edit the `.env` file and set these values:

```bash
# --- Required changes ---

# Enable the Microsoft connector
ENABLED_CONNECTORS=web,microsoft
COMPOSE_PROFILES=${ENABLED_CONNECTORS}

# Your domain (or VM public IP for testing)
APP_URL=https://omni.yourcompany.com
OMNI_DOMAIN=omni.yourcompany.com
ACME_EMAIL=admin@yourcompany.com

# Generate a strong encryption key and salt for credential storage
# The encryption key must be at least 32 characters
# The salt must be at least 16 characters
ENCRYPTION_KEY=<generate-a-random-string-of-at-least-32-characters>
ENCRYPTION_SALT=<generate-a-random-string-of-at-least-16-characters>

# Change the default database password
DATABASE_PASSWORD=<a-strong-random-password>
```

Generate secure random values:

```bash
# Generate ENCRYPTION_KEY (32+ characters)
openssl rand -base64 32

# Generate ENCRYPTION_SALT (16+ characters)
openssl rand -base64 16

# Generate DATABASE_PASSWORD
openssl rand -base64 24
```

### Start Omni

```bash
cd docker
docker compose up -d
```

This pulls all container images and starts the services. First run may take a few minutes.

Check that everything is running:

```bash
docker compose ps
```

You should see services including `omni-microsoft-connector` in the list.

---

## 13. Connect Microsoft 365 Sources in Omni

1. Open the Omni web UI in your browser:
   - `https://omni.yourcompany.com` (if DNS is configured), or
   - `http://<VM_PUBLIC_IP>:3000` (for testing without DNS/TLS)
2. Complete the initial setup wizard (create your admin account).
3. Navigate to **Settings** > **Connectors** (or **Sources**).
4. Click **Add Source** and select the Microsoft 365 service you want to connect:
   - **OneDrive** — indexes files from all users' OneDrive
   - **Outlook Mail** — indexes inbox messages for all users
   - **Outlook Calendar** — indexes calendar events for all users
   - **SharePoint** — indexes documents from all SharePoint sites
5. Enter the credentials from [steps 8–10](#8-register-an-app-in-microsoft-entra-id):
   - **Tenant ID:** `<your-tenant-id>`
   - **Client ID:** `<your-client-id>`
   - **Client Secret:** `<your-client-secret>`
6. Configure optional settings:
   - For **Calendar**: Adjust `calendar_past_months` and `calendar_future_months` (default: 6 each)
7. Click **Save** / **Connect**.
8. Repeat for each Microsoft 365 service you want to index.

> **Note:** All four Microsoft source types (OneDrive, Outlook, Calendar, SharePoint) use the **same** Azure App Registration credentials. You enter the same tenant ID, client ID, and client secret for each.

---

## 14. Verify the Integration

### Test the Connection

After adding a source, Omni will validate the credentials by calling the Microsoft Graph `/organization` endpoint. If the credentials are correct, the status will show as connected.

### Trigger a Sync

The connector manager will automatically schedule an initial full sync. You can also trigger it manually from the UI.

### Monitor Sync Progress

```bash
# View connector logs
docker compose logs -f microsoft-connector

# View connector manager logs
docker compose logs -f connector-manager
```

### What to Expect

- **OneDrive:** Iterates through all users and indexes their Drive files using delta queries
- **Outlook Mail:** Indexes inbox messages for all users
- **Outlook Calendar:** Indexes events within the configured time window (default: 6 months past to 6 months future)
- **SharePoint:** Discovers all sites in the tenant and indexes their document libraries

The first full sync may take significant time depending on data volume. Subsequent incremental syncs use Microsoft Graph delta tokens and are much faster.

---

## 15. Security Hardening Checklist

- [ ] **Restrict SSH access** to your IP or VPN range only (see [step 5](#5-configure-networking-nsg-rules))
- [ ] **Never expose internal ports** (5432/Postgres, 6379/Redis, 3001–3004, 4001–4009) in the NSG
- [ ] **Use HTTPS** with a real domain and TLS certificate
- [ ] **Rotate the client secret** before it expires — update the credential in Omni's UI when you do
- [ ] **Use strong, unique values** for `ENCRYPTION_KEY`, `ENCRYPTION_SALT`, and `DATABASE_PASSWORD`
- [ ] **Keep the VM updated:** `sudo apt update && sudo apt upgrade -y` regularly
- [ ] **Enable Azure Disk Encryption** for the OS and data disks
- [ ] **Set up Azure Backup** for the VM to protect against data loss
- [ ] **Monitor with Azure Monitor** — set alerts for VM CPU, memory, and disk usage
- [ ] **Use Application permissions (not Delegated)** — the connector is designed for app-only access with the minimum required read-only scopes

---

## 16. Troubleshooting

### "Insufficient privileges" when granting admin consent

You need the **Global Administrator** or **Application Administrator** role in Microsoft Entra ID. Ask your IT admin to grant consent, or have them assign you the required role.

### 401 Unauthorized errors in connector logs

- Verify `tenant_id`, `client_id`, and `client_secret` are correct
- Ensure admin consent has been granted (green checkmarks in API permissions)
- Check that the client secret hasn't expired
- The connector automatically retries once on 401 by refreshing the token

### 403 Forbidden errors

- Usually means a required API permission is missing or admin consent wasn't granted
- Go back to [step 9](#9-configure-microsoft-graph-api-permissions) and verify all five permissions show as "Granted"

### 429 Too Many Requests

- Normal under heavy sync load — the connector automatically respects `Retry-After` headers and retries indefinitely
- Microsoft Graph has per-tenant throttling limits; large tenants may need patience during the first full sync

### Connector container not starting

```bash
# Check if the microsoft profile is enabled
docker compose config --services | grep microsoft

# If missing, verify your .env has:
# ENABLED_CONNECTORS=web,microsoft
# COMPOSE_PROFILES=${ENABLED_CONNECTORS}
```

### No data appearing after sync

- Check connector logs: `docker compose logs microsoft-connector`
- Verify the connector-manager is routing correctly: `docker compose logs connector-manager`
- Ensure the indexer is running: `docker compose logs indexer`
- Confirm the app registration permissions match the source type (e.g., `Mail.Read` for Outlook)

---

## Quick Reference: What You'll Need

| Item | Where to Find It |
|------|-------------------|
| **Tenant ID** | Azure Portal > Microsoft Entra ID > Overview, or `az account show` |
| **Client ID** | Azure Portal > App registrations > Your app > Overview |
| **Client Secret** | Azure Portal > App registrations > Your app > Certificates & secrets |
| **VM Public IP** | Azure Portal > Virtual machines > Your VM > Overview, or output of `az vm create` |
| **Encryption Key** | Generate with `openssl rand -base64 32` |
| **Encryption Salt** | Generate with `openssl rand -base64 16` |

---

## Sources & References

- [Create an Azure Free Account](https://azure.microsoft.com/en-us/pricing/purchase-options/azure-account)
- [Azure Free Services](https://azure.microsoft.com/en-us/pricing/free-services)
- [Register an App with Microsoft Identity Platform](https://learn.microsoft.com/en-us/graph/auth-register-app-v2)
- [Configure API Permissions](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-configure-app-access-web-apis)
- [Azure Network Security Groups Overview](https://learn.microsoft.com/en-us/azure/virtual-network/network-security-groups-overview)
- [Docker Compose on Azure VM (Microsoft Learn)](https://learn.microsoft.com/en-us/previous-versions/azure/virtual-machines/linux/docker-compose-quickstart)
- [Entra ID App Registration for Microsoft Graph](https://laurakokkarinen.com/how-to-set-up-an-entra-id-application-registration-for-calling-microsoft-graph/)
- [Omni Documentation](https://docs.getomni.co)
