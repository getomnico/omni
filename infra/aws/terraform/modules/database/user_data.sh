#!/bin/bash
set -e

# Configure ECS cluster
echo "ECS_CLUSTER=${cluster_name}" >> /etc/ecs/ecs.config
echo "ECS_ENABLE_TASK_IAM_ROLE=true" >> /etc/ecs/ecs.config
echo "ECS_ENABLE_TASK_IAM_ROLE_NETWORK_HOST=true" >> /etc/ecs/ecs.config

# Wait for the EBS volume to be attached
echo "Waiting for EBS volume ${device_name} to be available..."
while [ ! -e ${device_name} ]; do
  sleep 1
done

# Check if the volume has a filesystem
if ! blkid ${device_name}; then
  echo "Creating filesystem on ${device_name}..."
  mkfs.ext4 ${device_name}
fi

# Create mount point
mkdir -p ${mount_point}

# Mount the volume
echo "Mounting ${device_name} to ${mount_point}..."
mount ${device_name} ${mount_point}

# Add to fstab for persistence across reboots
if ! grep -q "${device_name}" /etc/fstab; then
  echo "${device_name} ${mount_point} ext4 defaults,nofail 0 2" >> /etc/fstab
fi

# Set proper permissions for PostgreSQL
chown -R 999:999 ${mount_point}
chmod 700 ${mount_point}

echo "EBS volume setup complete!"
