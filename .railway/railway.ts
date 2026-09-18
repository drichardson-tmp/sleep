import { defineRailway, github, preserve, project, service } from "railway/iac";

export default defineRailway(() => {
  const api = service("sleep-telemetry", {
    source: github("drichardson-tmp/sleep", { branch: "main" }),
    // Railway detects the root Dockerfile and uses its release build/entrypoint.
    healthcheck: "/health",
    // The setpoint cache is process-local; use one replica per physical bed.
    replicas: 1,
    env: {
      SHARED_SECRET: preserve(),
      SLEEPME_API_TOKEN: preserve(),
      SLEEPME_DEVICE_ID: preserve(),
    },
  });

  return project("sleep", { resources: [api] });
});
