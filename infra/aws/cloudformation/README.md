  1. VPC (Virtual Private Cloud) - Lines 51-61

  Purpose: Creates an isolated network environment for all Omni resources.
  - CIDR Block 10.0.0.0/16: Provides 65,536 IP addresses for internal use
  - DNS Support/Hostnames enabled: Required for Service Discovery to work - allows ECS services to find each other by DNS names

  2. Internet Gateway - Lines 64-75

  Purpose: Enables internet connectivity for the VPC.
  - Allows resources in public subnets to communicate with the internet
  - Required for the ALB to receive traffic from users
  - Must be attached to VPC (AttachGateway resource)

  3. Public Subnets (2) - Lines 78-98

  Purpose: Host internet-facing resources (ALB).
  - Two subnets in different AZs: For high availability
  - MapPublicIpOnLaunch: Resources get public IPs automatically
  - Used for: Application Load Balancer only
  - CIDR: 10.0.1.0/24 and 10.0.2.0/24 (256 IPs each)

  4. Private Subnets (2) - Lines 101-119

  Purpose: Host internal resources that shouldn't be directly accessible from internet.
  - Two subnets in different AZs: Required for RDS Multi-AZ and high availability
  - No public IPs: Enhanced security
  - Used for: ECS services, RDS database, Redis cache
  - CIDR: 10.0.11.0/24 and 10.0.12.0/24

  5. NAT Gateway & Elastic IP - Lines 122-132

  Purpose: Allows private subnet resources to access internet for outbound connections.
  - Why needed: ECS services need to pull Docker images, call external APIs (Google, Slack, etc.), and access AWS services
  - One-way access: Allows outbound but prevents inbound connections from internet
  - EIP: Static IP address for the NAT Gateway

  6. Route Tables - Lines 135-188

  Public Route Table:

  - Routes all internet traffic (0.0.0.0/0) through Internet Gateway
  - Associated with both public subnets
  - Enables ALB to receive and send internet traffic

  Private Route Table:

  - Routes all internet traffic (0.0.0.0/0) through NAT Gateway
  - Associated with both private subnets
  - Allows ECS services to make outbound connections while staying protected

  7. Security Groups - Lines 191-251

  ALBSecurityGroup:

  - Purpose: Controls traffic to the Application Load Balancer
  - Allows: HTTP (80) and HTTPS (443) from anywhere (0.0.0.0/0)
  - Why: Users need to access the Omni web interface

  ECSSecurityGroup:

  - Purpose: Controls traffic for all ECS services
  - Allows:
    - Port 3000 from ALB (for web service)
    - Ports 3001-3003 from itself (inter-service communication)
  - Why: Services need to communicate with each other and receive traffic from ALB

  ECSInterServiceRule (Lines 220-227):

  - Purpose: Allows ECS services to talk to each other
  - Why separate: Avoids circular dependency in CloudFormation
  - Ports 3001-3003: Searcher, Indexer, and AI services

  DatabaseSecurityGroup:

  - Purpose: Controls access to PostgreSQL database
  - Allows: Port 5432 only from ECS services
  - Why: Only application services should access the database

  RedisSecurityGroup:

  - Purpose: Controls access to Redis cache
  - Allows: Port 6379 only from ECS services
  - Why: Only application services should access Redis

  8. Subnet Groups - Lines 254-298

  DBSubnetGroup:

  - Purpose: Tells RDS which subnets to use for database deployment
  - Why: RDS requires subnets in at least 2 AZs for failover capability
  - Uses: Both private subnets

  CacheSubnetGroup:

  - Purpose: Tells ElastiCache which subnets to use for Redis
  - Why: ElastiCache needs to know where to place Redis nodes
  - Uses: Both private subnets

  Architecture Summary

  The networking setup creates a secure, scalable architecture:

  1. Internet Traffic Flow: Internet → ALB (public subnet) → ECS Services (private subnet)
  2. Outbound Traffic: ECS Services → NAT Gateway → Internet (for external APIs)
  3. Internal Communication: Services use private IPs and Service Discovery DNS
  4. Database Access: Only ECS services can reach RDS/Redis (security groups)
  5. High Availability: Resources spread across 2 Availability Zones
  6. Security Layers: Private subnets + Security Groups + NAT Gateway provide defense in depth


