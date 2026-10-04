# Hoglet user documentation

Hoglet is a PostHog-compatible analytics backend: one binary, one data
directory. Point your PostHog SDKs at it by changing `api_host`.

Start here:

1. [Quickstart](quickstart.md): install, first run, first event.
2. [SDK setup](sdk-setup.md): posthog-js, React/Next.js, posthog-node, posthog-python, curl, first-party proxy, feature flags.
3. [Deploy](deploy.md): VPS, systemd, Docker, TLS, upgrades.

Then, as needed:

- [Migrate from PostHog](migrate-from-posthog.md): shadow mode, import, reconcile, cut-over, rollback.
- [Data and backup](data-and-backup.md): what is on disk, backup, restore, retention, erasure, querying Parquet.
- [Configuration](configuration.md): every environment variable and CLI command.
- [Insights and queries](insights-and-queries.md): what each insight computes, the SQL tab.
- [Feature flags](feature-flags.md): model, evaluation, local evaluation.
- [API](api.md): authentication, scopes, errors, response codes.
- [FAQ](faq.md) and [troubleshooting](troubleshooting.md).

Conventions in these pages:

- `https://analytics.example.com` is your Hoglet address. Replace it.
- `phc_...` is a project token (safe in client code). `phx_...` is a personal API key (secret).
- Commands were run against a build of this repository unless a page says otherwise.
- Performance numbers appear only where they are recorded in the repository's `claims.md` or `ROADMAP.md`.

API reference for your running instance: `/docs` (interactive) and `/openapi.json`.
