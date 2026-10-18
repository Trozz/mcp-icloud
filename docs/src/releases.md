# Releases

- Work happens on `feature/*` and `bugfix/*` branches merged into `develop` by pull request.
- Every merge into `develop` that touches the server publishes a **pre-release** `vX.Y.Z-dev.N`: a GitHub pre-release and image tags `X.Y.Z-dev.N` and `develop`.
- A pull request from `develop` into `main` publishes a **release** `vX.Y.Z` when merged: image tags `X.Y.Z`, `X.Y`, `X` and `latest`, binaries for Linux x86_64 and arm64 and macOS arm64 with checksums, and this documentation site.
- The version bump comes from the `develop` to `main` pull request: a `major`, `minor` or `patch` label, otherwise `[major]`, `[minor]` or `[patch]` in its title, otherwise patch.
- Only merge commits are allowed, so `develop` and `main` never diverge. Pull requests into `main` must come from `develop` in this repository.

Images are published to `ghcr.io/trozz/mcp-icloud`. Pin a version tag such as `ghcr.io/trozz/mcp-icloud:1.2.3` in production, and read the release notes before upgrading.

## Repository setup (maintainers, once)

```bash
gh label create major --color B60205 --description "Release: major version bump"
gh label create minor --color FBCA04 --description "Release: minor version bump"
gh label create patch --color 0E8A16 --description "Release: patch version bump"
```

- Under **Settings > Actions > General > Workflow permissions**, the workflows must be allowed to grant `contents: write`, `packages: write`, `id-token: write`, `attestations: write` and `pages: write` (the jobs request them explicitly).
- Under **Settings > Pages**, the source must be **GitHub Actions** for this site to publish.
- After the first image is published, set the `mcp-icloud` package visibility to public under **Packages** for anonymous pulls.
