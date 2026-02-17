#!/usr/bin/env bash
set -euo pipefail

# Deploys Omni to GCP using Terraform.
# Usage: ./deploy.sh [plan|apply|destroy]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TF_DIR="${SCRIPT_DIR}/.."
ACTION="${1:-plan}"

if [ ! -f "${TF_DIR}/terraform.tfvars" ]; then
  echo "Error: terraform.tfvars not found."
  echo "Copy terraform.tfvars.example to terraform.tfvars and fill in your values."
  exit 1
fi

echo "=== Terraform Init ==="
terraform -chdir="${TF_DIR}" init

echo ""
echo "=== Terraform ${ACTION} ==="
case "${ACTION}" in
  plan)
    terraform -chdir="${TF_DIR}" plan
    ;;
  apply)
    terraform -chdir="${TF_DIR}" apply
    ;;
  destroy)
    terraform -chdir="${TF_DIR}" destroy
    ;;
  *)
    echo "Unknown action: ${ACTION}"
    echo "Usage: $0 [plan|apply|destroy]"
    exit 1
    ;;
esac
