#!/usr/bin/env bash
set -euo pipefail

# Creates a GCS bucket for Terraform remote state storage.
# Usage: ./init-backend.sh <PROJECT_ID> <CUSTOMER_NAME> [REGION]

PROJECT_ID="${1:?Usage: $0 <PROJECT_ID> <CUSTOMER_NAME> [REGION]}"
CUSTOMER_NAME="${2:?Usage: $0 <PROJECT_ID> <CUSTOMER_NAME> [REGION]}"
REGION="${3:-us-central1}"

BUCKET="omni-${CUSTOMER_NAME}-terraform-state"

echo "Creating GCS bucket gs://${BUCKET} in project ${PROJECT_ID}..."
gsutil mb -p "${PROJECT_ID}" -l "${REGION}" "gs://${BUCKET}" 2>/dev/null || echo "Bucket already exists."

echo "Enabling versioning..."
gsutil versioning set on "gs://${BUCKET}"

echo ""
echo "Backend bucket ready. Create infra/gcp/terraform/backend.tf with:"
echo ""
echo 'terraform {'
echo '  backend "gcs" {'
echo "    bucket = \"${BUCKET}\""
echo '    prefix = "terraform/state"'
echo '  }'
echo '}'
