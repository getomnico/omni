#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TF_DIR="${SCRIPT_DIR}/.."

echo "=== Terraform Format Check ==="
terraform -chdir="${TF_DIR}" fmt -check -recursive

echo ""
echo "=== Terraform Init ==="
terraform -chdir="${TF_DIR}" init -backend=false

echo ""
echo "=== Terraform Validate ==="
terraform -chdir="${TF_DIR}" validate

echo ""
echo "All checks passed."
