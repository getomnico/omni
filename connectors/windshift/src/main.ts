import {
  WindshiftConnector,
  refreshWindshiftServerConfig,
} from "./connector.js";

// Load the admin-configured Windshift server before serving so the first
// manifest registration already carries the OAuth endpoints. Non-fatal: on
// failure the connector falls back to WINDSHIFT_BASE_URL env vars.
await refreshWindshiftServerConfig();

const connector = new WindshiftConnector();
connector.serve();

// Re-read the server config so admin UI changes propagate without a container
// restart. Matches the SDK's 30s manifest re-registration cadence.
setInterval(() => {
  void refreshWindshiftServerConfig(true);
}, 30_000);
