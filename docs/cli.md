# Omni CLI

The `omni` CLI manages self-hosted Omni Docker Compose deployments. v1 supports Docker Compose only.

## Install

Download the binary for your platform from the Omni GitHub release assets:

- `omni-linux-x86_64`
- `omni-macos-arm64`
- `omni-windows-x86_64.exe`

Then place it on your `PATH` and make it executable on Unix:

```bash
chmod +x omni-linux-x86_64
sudo mv omni-linux-x86_64 /usr/local/bin/omni
omni --version
```

## Upgrade

Run from your Omni Docker Compose install directory, or pass `--install-dir`:

```bash
omni upgrade
```

By default, `omni upgrade` uses the latest stable GitHub release. To target a specific release:

```bash
omni upgrade --to v0.1.7
# or
omni upgrade --to 0.1.7
```

The upgrade command:

1. downloads the target release's `omni-docker-compose.tar.gz`;
2. backs up `.env` and managed deployment files under `.omni/backups/<timestamp>/`;
3. updates managed files such as `docker/docker-compose.yml`, `.env.example`, and `Caddyfile`;
4. updates `OMNI_VERSION` in `.env` to the release image tag;
5. warns about variables missing from `.env` and prompts you to append new values;
6. warns about variables in your `.env` that no longer appear in `.env.example`;
7. runs `docker compose pull` and `docker compose up -d --remove-orphans`;
8. runs a doctor summary.

Useful flags:

```bash
omni upgrade --dry-run       # preview changes only
omni upgrade --yes           # accept prompts with defaults
omni upgrade --force         # continue despite detected local edits to managed files
omni upgrade --skip-pull     # update files without pulling images
omni upgrade --skip-up       # pull/update files without recreating services
```

## Managed Compose files and customization

Omni's Compose files are vendor-managed release assets. The CLI replaces these files during upgrades instead of attempting to merge YAML. This keeps upgrades predictable because service definitions, anchors, profiles, health checks, and image tags often change together.

Do not customize `docker/docker-compose.yml` directly. Put local changes in user-owned override files instead, for example:

- `docker/docker-compose.override.yml`
- `docker-compose.override.yml`

The CLI includes these override files in its Compose commands and never overwrites them.

## Environment variables

`.env.example` is the canonical template for a release. During upgrade, the CLI preserves your existing `.env` values, updates `OMNI_VERSION`, and prompts for variables that exist in the target `.env.example` but not in your `.env`.

Missing variables are warnings, not hard errors, because not every variable is required in every deployment.

Preview env changes without upgrading:

```bash
omni env diff
omni env diff --to v0.1.7
```

## Doctor

Run diagnostics:

```bash
omni doctor
omni doctor --verbose
omni doctor --json
```

`doctor` checks local deployment files, Docker and Docker Compose availability, Compose service state, container health, image tags, service health endpoints, connector-manager source/connector status where reachable, recent sync run status, and recent logs for suspicious errors.

## Other commands

```bash
omni version             # CLI, configured Omni version, and running Omni image tags
omni version --json      # machine-readable version info
omni status              # concise docker compose ps view
omni logs web --tail 100 # service logs
omni backup              # back up .env and managed files
omni compose -- ps       # passthrough to docker compose using Omni files
```
