#!/usr/bin/env bash
set -euo pipefail

# Dev Stack Manager
# Provides temporal isolation for local omni development across git worktrees.
# Only one stack may be active at a time to conserve RAM.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
LOCK_DIR="${HOME}/.omni"
LOCK_FILE="${LOCK_DIR}/dev-stack.lock"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

usage() {
    cat <<EOF
Usage: $(basename "$0") <command>

Commands:
  init              Create .env.<branch> from .env.example for this worktree
  start             Start this worktree's stack (enforces single active stack)
  stop              Stop this worktree's stack and release the lock
  status            Show active stack owner and running containers
  logs [service]    Tail logs for the given service (or all if omitted)

Environment:
  OMNI_WORKTREE is derived from the current git branch.
EOF
    exit 1
}

get_branch() {
    git -C "${REPO_ROOT}" rev-parse --abbrev-ref HEAD 2>/dev/null || true
}

sanitize_name() {
    local raw="$1"
    # Replace non-alphanumeric with dashes, collapse multiple dashes, trim ends
    echo "${raw}" | sed -E 's/[^a-zA-Z0-9]+/-/g; s/^-+//; s/-+$//'
}

get_env_file() {
    local branch
    branch="$(get_branch)"
    if [[ -z "${branch}" ]]; then
        echo ".env"
    else
        echo ".env.${branch}"
    fi
}

get_project_name() {
    local branch
    branch="$(get_branch)"
    if [[ -z "${branch}" ]]; then
        echo "omni"
    else
        echo "omni-$(sanitize_name "${branch}")"
    fi
}

compose_cmd() {
    local env_file
    env_file="$(get_env_file)"
    docker compose \
        -f "${REPO_ROOT}/docker/docker-compose.yml" \
        -f "${REPO_ROOT}/docker/docker-compose.dev.yml" \
        --env-file "${REPO_ROOT}/${env_file}" \
        "$@"
}

# ---------------------------------------------------------------------------
# Lockfile operations
# ---------------------------------------------------------------------------

acquire_lock() {
    mkdir -p "${LOCK_DIR}"
    local worktree_path branch
    worktree_path="$(cd "${REPO_ROOT}" && pwd)"
    branch="$(get_branch)"

    if [[ -f "${LOCK_FILE}" ]]; then
        local locked_path locked_branch
        locked_path="$(head -n1 "${LOCK_FILE}" 2>/dev/null || true)"
        locked_branch="$(sed -n '2p' "${LOCK_FILE}" 2>/dev/null || true)"

        if [[ "${locked_path}" != "${worktree_path}" ]]; then
            echo "ERROR: Another dev stack is already active." >&2
            echo "  Worktree: ${locked_path}" >&2
            echo "  Branch:   ${locked_branch}" >&2
            echo "Run 'dev-stack.sh stop' in that worktree first." >&2
            exit 1
        fi
    fi

    printf '%s\n%s\n%d\n' "${worktree_path}" "${branch}" "$(date +%s)" > "${LOCK_FILE}"
}

release_lock() {
    local worktree_path
    worktree_path="$(cd "${REPO_ROOT}" && pwd)"

    if [[ -f "${LOCK_FILE}" ]]; then
        local locked_path
        locked_path="$(head -n1 "${LOCK_FILE}" 2>/dev/null || true)"
        if [[ "${locked_path}" == "${worktree_path}" ]]; then
            rm -f "${LOCK_FILE}"
        else
            echo "WARNING: Lock is owned by another worktree (${locked_path}). Not releasing." >&2
        fi
    fi
}

read_lock() {
    if [[ -f "${LOCK_FILE}" ]]; then
        local locked_path locked_branch locked_at
        locked_path="$(head -n1 "${LOCK_FILE}" 2>/dev/null || true)"
        locked_branch="$(sed -n '2p' "${LOCK_FILE}" 2>/dev/null || true)"
        locked_at="$(sed -n '3p' "${LOCK_FILE}" 2>/dev/null || true)"
        echo "${locked_path}"$'\t'"${locked_branch}"$'\t'"${locked_at}"
    fi
}

# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------

cmd_init() {
    local branch env_file project_name example_env
    branch="$(get_branch)"
    if [[ -z "${branch}" ]]; then
        echo "ERROR: Not in a git repository or detached HEAD." >&2
        exit 1
    fi

    env_file="$(get_env_file)"
    project_name="$(get_project_name)"
    example_env="${REPO_ROOT}/.env.example"

    if [[ ! -f "${example_env}" ]]; then
        echo "ERROR: .env.example not found at ${example_env}" >&2
        exit 1
    fi

    if [[ -f "${REPO_ROOT}/${env_file}" ]]; then
        echo "Env file already exists: ${env_file}"
        read -r -p "Overwrite? [y/N] " reply
        if [[ "${reply}" != "y" && "${reply}" != "Y" ]]; then
            echo "Aborted."
            exit 0
        fi
    fi

    cp "${example_env}" "${REPO_ROOT}/${env_file}"
    # Prepend OMNI_WORKTREE if not already present
    if ! grep -q '^OMNI_WORKTREE=' "${REPO_ROOT}/${env_file}"; then
        {
            echo ""
            echo "# Worktree isolation — managed by dev-stack.sh"
            echo "OMNI_WORKTREE=${project_name}"
        } >> "${REPO_ROOT}/${env_file}"
    fi

    echo "Created ${env_file} with OMNI_WORKTREE=${project_name}"
}

cmd_start() {
    local env_file
    env_file="$(get_env_file)"

    if [[ ! -f "${REPO_ROOT}/${env_file}" ]]; then
        echo "ERROR: Env file not found: ${REPO_ROOT}/${env_file}" >&2
        echo "Run '$(basename "$0") init' first." >&2
        exit 1
    fi

    acquire_lock

    echo "Starting dev stack: $(get_project_name)"
    compose_cmd up -d --build

    echo ""
    echo "Stack started. Running database seed..."
    "${SCRIPT_DIR}/seed-db.sh" "$(get_project_name)"

    echo ""
    echo "Done. Access the app at http://localhost:${WEB_PORT:-3000}"
}

cmd_stop() {
    local env_file
    env_file="$(get_env_file)"

    if [[ ! -f "${REPO_ROOT}/${env_file}" ]]; then
        echo "WARNING: Env file not found: ${env_file}. Stopping via project name fallback." >&2
    fi

    echo "Stopping dev stack: $(get_project_name)"
    compose_cmd down --remove-orphans || true
    release_lock
    echo "Stopped."
}

cmd_status() {
    local lock_info
    lock_info="$(read_lock)"

    echo "=== Dev Stack Lock ==="
    if [[ -n "${lock_info}" ]]; then
        local locked_path locked_branch locked_at
        locked_path="$(echo "${lock_info}" | cut -f1)"
        locked_branch="$(echo "${lock_info}" | cut -f2)"
        locked_at="$(echo "${lock_info}" | cut -f3)"
        locked_at_human="$(date -d "@${locked_at}" 2>/dev/null || date -r "${locked_at}" 2>/dev/null || echo "${locked_at}")"
        echo "Active:  YES"
        echo "Path:    ${locked_path}"
        echo "Branch:  ${locked_branch}"
        echo "Since:   ${locked_at_human}"
    else
        echo "Active:  NO"
    fi

    echo ""
    echo "=== Running omni containers ==="
    docker ps --filter "name=^/$(get_project_name)" --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}" 2>/dev/null || true
}

cmd_logs() {
    local service="${1:-}"
    if [[ -n "${service}" ]]; then
        compose_cmd logs -f "${service}"
    else
        compose_cmd logs -f
    fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

[[ $# -eq 0 ]] && usage

COMMAND="$1"
shift || true

case "${COMMAND}" in
    init)   cmd_init ;;
    start)  cmd_start ;;
    stop)   cmd_stop ;;
    status) cmd_status ;;
    logs)   cmd_logs "$@" ;;
    *)      usage ;;
esac
