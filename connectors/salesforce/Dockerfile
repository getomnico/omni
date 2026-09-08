# Salesforce Connector Dockerfile

# The official Salesforce CLI and MCP server require Node >=22. Keep them in
# a dedicated stage so the runtime image contains no npm/build toolchain.
FROM node:22-bookworm-slim AS salesforce-tools
RUN npm install --global --no-audit --no-fund \
    @salesforce/cli@2.150.6 \
    @salesforce/mcp@0.30.15

FROM python:3.11-slim AS builder

WORKDIR /build

# Install build dependencies
RUN pip install --no-cache-dir hatchling

# Copy and build the SDK
COPY sdk/python /sdk/python
RUN pip wheel --no-deps -w /wheels /sdk/python

# Copy and build the connector
COPY connectors/salesforce /build
RUN pip wheel --no-deps -w /wheels /build

# Production stage
FROM python:3.11-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libstdc++6 \
    && rm -rf /var/lib/apt/lists/*

# Copy the Node runtime and the pinned official Salesforce tools.
COPY --from=salesforce-tools /usr/local/bin/node /usr/local/bin/node
COPY --from=salesforce-tools /usr/local/bin/sf /usr/local/bin/sf
COPY --from=salesforce-tools /usr/local/bin/sf-mcp-server /usr/local/bin/sf-mcp-server
COPY --from=salesforce-tools /usr/local/lib/node_modules /usr/local/lib/node_modules
# npm's generated global shims use paths relative to the shim itself. Keep
# package-relative symlinks so those imports resolve inside the copied module.
RUN rm -f /usr/local/bin/sf /usr/local/bin/sf-mcp-server \
    && ln -s /usr/local/lib/node_modules/@salesforce/cli/bin/run.js /usr/local/bin/sf \
    && ln -s /usr/local/lib/node_modules/@salesforce/mcp/bin/run.js /usr/local/bin/sf-mcp-server

# Copy wheels and install
COPY --from=builder /wheels /wheels
RUN pip install --no-cache-dir /wheels/*.whl && rm -rf /wheels

# Copy entry point and the per-source auth-isolating MCP launcher.
COPY connectors/salesforce/main.py /app/
COPY connectors/salesforce/bin/omni-salesforce-mcp /usr/local/bin/omni-salesforce-mcp
RUN chmod 755 /usr/local/bin/omni-salesforce-mcp

ENV PYTHONUNBUFFERED=1

CMD ["python", "main.py"]
