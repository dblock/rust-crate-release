# GitHub Actions and Trusted Publishing

Follow [README Usage](../README.md#usage) for setup. This guide adds permissions, troubleshooting, and customization details. Examples use crate `widget`, repository `acme/widget`, and branch `main`; replace these with yours.

## Prepare Your Project

Commit your intended `Cargo.toml` version, `Cargo.lock`, and caller workflow to the default branch. Run `cargo generate-lockfile` if the lockfile is missing, including for a library. No local rcr installation or `rcr.toml` is required.

For a workspace member, set `manifest = "path/to/member/Cargo.toml"` in an optional `rcr.toml`. Each release handles one package with its own explicit version; workspace-inherited versions and coordinated multi-crate releases are not supported.

No changelog is required. If you keep one, add meaningful unreleased notes before releasing; empty notes are rejected. See the [README example](../README.md#4-update-changelog) or [Changelog Editing](changelogs.md) for automatic detection, supported formats, and disabling file updates.

The workflow runs formatting checks, tests, and Clippy before publishing. Use [`setup-command`](#customize-release-phases) for project dependencies and tools; use the [composite action](#custom-jobs-with-the-composite-action) only when you need a custom job layout.

## Publish a New Crate Once

**Skip this section if your crate is already published.** [crates.io requires an API token for the first publication](https://crates.io/docs/trusted-publishing); configure trusted publishing afterward.

Use Cargo locally for this one-time bootstrap. The supplied Actions workflow supports trusted publishing only; do not add a Cargo-token secret to it.

1. Sign in to crates.io with GitHub and verify your email under [Account Settings](https://crates.io/settings/profile), as described in Cargo's [first-publication prerequisites](https://doc.rust-lang.org/cargo/reference/publishing.html#before-your-first-publish).
2. Create a short-lived token under [API Tokens](https://crates.io/settings/tokens). Grant `publish-new` for creating the crate and restrict the crate-name pattern to your intended name. Do not share the token or commit it to Git.
3. Review the intended version and publishing metadata in `Cargo.toml`, and commit it with `Cargo.lock` and the source you intend to publish. From the project root, log in interactively and check the package:

   ```sh
   cargo login
   cargo publish --locked --dry-run
   ```

   Paste the token only at Cargo's prompt. Cargo saves it through its configured credential provider; never put the token in a command, shell history, Git, or GitHub secrets.
4. Inspect the dry-run result and generated archive, then publish with Cargo's saved credentials:

   ```sh
   cargo publish --locked
   ```

5. Run `cargo logout` even after failure when credentials are no longer needed, and revoke the token under crates.io **API Tokens**. Confirm whether the version reached crates.io before retrying a failed upload.
6. Prepare the next version: after publishing `0.1.0`, change `Cargo.toml` to `0.1.1`, run `cargo check` to refresh `Cargo.lock`, add unreleased notes if changelog updates are enabled, and commit. Cargo does not do these steps or create Git tags for you. You can optionally tag the original published commit as `v0.1.0`.
7. Configure trusted publishing below. Subsequent releases use the workflow, not a local token.

## Configure GitHub

In your repository's **Settings > Environments**, create `release`. Restrict deployments to your default branch, such as `main`; add required reviewers if desired. The supplied workflow selects this environment for you.

The workflow pushes a release commit and `vX.Y.Z` tag, then a development commit. It does not open version-update PRs. Check that your repository allows these ordinary, non-force pushes: `contents: write` does not bypass branch or tag rules.

**If your default branch requires pull requests, the release push will be blocked.** With no bypass actors, that requirement must be removed to use this direct-push workflow. This also allows direct pushes by other writers, not only rcr, so review the tradeoff before adopting it. Keep force-push and deletion protections. Status-check, signing, restricted-push, and tag rules must also allow the generated commits and tags.

Review these settings before publishing: a crate upload can succeed before a blocked Git push fails. rcr does not change repository protections for you.

Ensure organization policies allow this reusable workflow and its actions. No repository Cargo-token secret, personal access token, or `secrets: inherit` setting is needed for the standard setup.

## Configure the crates.io Publisher

On crates.io, open your crate's **Settings**, for example `https://crates.io/crates/widget/settings`. Under **Trusted Publishing**, click **Add**, choose **GitHub**, and fill in:

| Field | Example Value | Meaning |
| --- | --- | --- |
| Repository owner | `acme` | Your GitHub user or organization. |
| Repository name | `widget` | Your consumer repository, not `rust-crate-release`. |
| Workflow filename | `release.yml` | Your caller file's basename, not its full path. |
| Environment name (optional) | `release` | The GitHub environment configured above; enter it to restrict the publisher to that environment. |

Click **Add** to save, then check the listed repository, workflow, and environment. You must be a crate owner with a verified email and linked GitHub account. Use `release` for the environment even though the form labels it optional.

Use **your** repository and `release.yml`, not `dblock/rust-crate-release`. The crate name can differ from the repository name. Each crate needs its own publisher configuration; adding a repository URL to `Cargo.toml` does not register one.

rcr handles authentication automatically using GitHub's identity service (OIDC). It obtains a short-lived crates.io token and revokes it after the upload attempt. Do not add a separate authentication action or pass a Cargo token to the workflow.

## Add the Release Workflow

The [README workflow](../README.md#1-add-the-release-workflow) is enough for normal use. To also accept a manually entered version, use this variant at `.github/workflows/release.yml`:

```yaml
name: Release

on:
  workflow_dispatch:
    inputs:
      version:
        description: Optional explicit stable version
        required: false
        default: ''
        type: string

permissions:
  actions: read
  contents: write
  id-token: write

jobs:
  release:
    uses: dblock/rust-crate-release/.github/workflows/release.yml@v0.1.2
    with:
      tool-ref: v0.1.2
      version: ${{ inputs.version }}
```

Both refs select the tool's release, not your crate's version. Keep `uses` and `tool-ref` pinned to the same reviewed tag or commit. The optional `version` input selects your crate's release version; when empty, numeric releases use the manifest version. An optional `config` input selects another configuration file relative to your project root.

The reusable workflow supplies an Ubuntu GitHub-hosted runner, full-history consumer checkout, sibling tooling checkout, bot commit identity, the `release` environment, and non-canceling repository/ref concurrency. It runs only for manual dispatch on the actual default branch. It exposes `version` and JSON `verification` outputs.

| Permission | Purpose |
| --- | --- |
| `actions: read` | Independently download and verify the durable recovery artifact before publication. |
| `contents: write` | Push release/development commits and tags, and create GitHub Releases when enabled. |
| `id-token: write` | Request the OIDC identity accepted by the crates.io publisher. |

Merge the caller workflow and any optional configuration onto the default branch. Do not use a tag-push trigger: rcr creates the release tag after crate publication.

## Run and Verify a Release

Commit any intended manifest-version change and add meaningful notes if handwritten changelog updates are enabled. Ensure the default branch is clean, up to date, and has no competing release. Then select **Actions > Release > Run workflow**, choose the default branch, and leave `version` empty unless deliberately overriding the manifest version.

The equivalent GitHub CLI command is:

```sh
gh workflow run release.yml --repo acme/widget --ref main
```

To explicitly select a higher stable version with the optional input above, use `-f version=0.2.0`. **Running the workflow publishes a real release**, not a dry run.

rcr publishes the version in `Cargo.toml`, not a version incremented beforehand. With `0.1.1` in the manifest, it publishes `0.1.1`, creates tag `v0.1.1` and a GitHub Release, then commits and pushes `0.1.2` for development. Check these results in the run summary, on crates.io, and in your repository. For a minor or major release, commit the intended manifest version and updated lockfile before dispatching.

If the run fails, [check for partial success](recovery.md) before trying again. A published crate cannot be overwritten.

### Download and Verify Release Files

The workflow saves the checked archive before publishing and verifies that the uploaded recovery files match. You do not need to manage these files during a normal release.

Recovery files appear as `rcr-release-<run-id>-<attempt>` and final diagnostics as `rcr-diagnostics-<run-id>-<attempt>`. To download the immutable pre-publication snapshot outside your checkout and verify a release independently, replace the example IDs/version:

```sh
gh run download RUN_ID --repo acme/widget \
  --name rcr-release-RUN_ID-RUN_ATTEMPT --dir ../widget-release-artifacts
rcr verify --name widget --release-version 0.1.1 \
  --archive ../widget-release-artifacts/widget-0.1.1.crate
```

Registry visibility can lag upload acceptance. A `not_visible` verification result is a successful but unverified release outcome; it does not prevent hosting/development or authorize republishing. Run the read-only verification command later. Authentication, transport, metadata, or checksum errors remain failures. See [Diagnostics and Recovery](recovery.md) before retrying partial success; do not blindly dispatch again after a failed push or hosting step.

## Troubleshooting

| Symptom | What to Do |
| --- | --- |
| Workflow is skipped | Run it on your default branch using a GitHub-hosted runner. |
| Authentication fails | Check that the crate exists and its saved publisher matches your repository, `release.yml`, and `release` environment. Confirm `id-token: write`. |
| Stored-token guard fails | Remove `CARGO_REGISTRY_TOKEN` and any separate authentication action from the job. |
| Artifact verification fails | Check `actions: read` and the upload step. Do not bypass verification. |
| Git push fails after publication | Check crates.io and remote tags before retrying; review [branch rules](#configure-github). |
| Formatting, tests, or packaging fail | Read the failing command's output. Check the lockfile and any required tools or system libraries. |
| Run appears stuck | Check the active step. Dependency compilation and repeated source/archive/development validation can take several minutes; child-command output may appear only when it finishes. Do not dispatch a competing release. |
| Verification says `not_visible` | Wait, then [verify the released archive](recovery.md#verify-without-publishing); do not upload again. |

The sections below are advanced reference. They are not additional setup steps.

## Custom Jobs with the Composite Action

Use `dblock/rust-crate-release@v0.1.2` for custom job layouts beyond the reusable workflow's `setup-command` hook. A composite cannot impose job-level environment, permissions, concurrency, or checkout policy. Supply them explicitly:

```yaml
name: Release

on:
  workflow_dispatch:

permissions:
  actions: read
  contents: write
  id-token: write

concurrency:
  group: rcr-${{ github.repository }}-${{ github.ref }}
  cancel-in-progress: false

jobs:
  release:
    name: Release Crate
    if: github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    environment: release
    steps:
      - name: Check Out Source
        uses: actions/checkout@v7.0.1
        with:
          fetch-depth: 0
      - name: Configure Release Identity
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
      - name: Release Crate
        uses: dblock/rust-crate-release@v0.1.2
```

Insert your additional setup before the final step. The action accepts `root` (default `.`), `config`, and `version`; outputs are `name`, `version`, `archive`, `checksum`, and JSON `verification`. Keep the selected Cargo package inside its Git root and do not leave untracked tooling inside the checkout.

The action installs the selected Rust toolchain with rustfmt and Clippy, restores Cargo dependency caches for the tool and project, and builds rcr from the reviewed source. Cached builds never replace release artifacts or publication verification. Your project must support the selected toolchain. Packaging checks run offline, so custom validators must first fetch dependencies and install required tools; normal Cargo test/Clippy runs populate the dependency cache.

## Preservation and Failure Boundaries

The Rust preparation command saves immutable source inputs under Git metadata and completes validation, packaging, and bundle creation without publication. A separate standard upload action preserves only the exact crate, recovery bundle, source inputs, and known diagnostics. This action receives artifact-service credentials that GitHub does not expose to composite shell steps; rcr does not reimplement that private upload protocol.

Before registry mutation, Rust reads the immutable artifact through GitHub's public [artifact REST API](https://docs.github.com/en/rest/actions/artifacts). It checks ID/name/run/attempt/expiry, stored size and optional SHA-256 digest, ZIP entries, and exact retained crate/bundle/source-input bytes. Missing, non-regular, or mismatching required files block publication. ZIP downloads are bounded to 1 GiB and individual diagnostics to 2 MiB. A supplied artifact ID or successful step alone is not proof of preservation.

Names are immutable within a run/attempt; conflicts fail rather than overwrite recovery evidence. A new attempt captures fresh bound inputs instead of restoring an older attempt's state. Deliberate same-attempt retries reverify the snapshot and reconcile actual outputs. Downloaded diagnostics are never interpreted as skip instructions. Repository tokens never follow signed download redirects, and raw tokens, signed URLs, and service bodies are not echoed.

## Customize Release Phases

The reusable workflow accepts `toolchain` (default `1.99.1`, which is the minimum), `git-user-name`, and `git-user-email`. These select the project/tool runtime and release commit identity without editing the shared workflow. A composite-action caller configures its own Git identity.

Both entry points accept an optional `setup-command` as JSON argv. For a project-owned Rust setup task, add this beside `tool-ref` in the reusable-workflow caller:

```yaml
setup-command: '["cargo", "xtask", "setup"]'
```

The command runs once in the project root after Rust/tool installation and the caller guard, before preparation or validation. Use it to install your dependencies and tools; other consumers do not inherit them. A failure blocks preparation and publication and selects the failure callback. Setup must leave the source tree clean. Empty input skips setup; shell syntax requires an explicit shell program in argv.

Both entry points accept direct JSON argv inputs: `validate-command` replaces the configured validation argv, `test-command` appends an additional source/development validation command, `prepare-command` edits files after release metadata preparation, and `bump-command` edits files after optional next-development metadata preparation. Edit commands finish before trusted staging and real validation. Their environment provides `RCR_RELEASE_VERSION` and, for development, `RCR_NEXT_VERSION`. Omitted/empty inputs retain existing behavior. `prepare-command`, `publish-command`, and `bump-command` also accept TOML argv configuration; `RCR_*_COMMAND` environment selections take precedence. Shell syntax requires an explicit shell program in argv.

`publish-command` replaces only registry upload, after immutable artifact preservation and release-ref checks. It receives the exact retained `CRATE_ARCHIVE` and release version. On success, rcr independently requires that version's registry checksum and downloaded bytes to match before Git push; a matching existing version skips the command. No successful exit or callback flag alone proves publication. The command owns its authentication and must make the version visible before returning; otherwise reconcile later with the original inputs rather than automatically retrying an ambiguous upload. The default SDK publisher remains recommended for crates.io OIDC.

These commands cannot replace the mandatory archive consumer check, source/commit identity checks, independent remote artifact verification, or final verification. Use a typed project runner for more elaborate provider composition.

## Terminal Callbacks

The composite and reusable workflows accept `success-command` and `failure-command` as JSON argv, for example `["cargo","run","--locked","--example","notify","--"]`. Alternatively configure `success = ["program", "argument"]` and `failure = ["program", "argument"]` in `rcr.toml`. Commands execute directly in the project root without implicit shell evaluation. The terminal `always()` step covers preparation, external preservation, publication, and diagnostic-upload failures without reporting success twice. Ordinary local `rcr release` uses the same configured callbacks; preparation alone does not run them.

Callbacks receive `RCR_STATUS` and `RCR_EXECUTION_CONTEXT`, a JSON execution snapshot. It contains allowlisted run/ref/SHA/job/workflow/event fields and actual external step `outcome`/`conclusion` values, including continued failures, skipped steps, and cancellation. Step outputs and arbitrary environment variables are never included. External step IDs are a map projection, not a chronological history; the engine's own ordered `history` remains separate. These observations cannot authorize skipping release verification.

If Rust setup or tool installation fails, an explicit `failure-command` input still runs through the hosted Ubuntu runner's Bash/jq argv adapter, with the same filtered context. Configuration-only callbacks require the installed tool; checkout failure or cancellation can also prevent a project program from being available. Keep notification commands independent of the failing setup where necessary. A failing failure callback cannot change an already-failed release into success or replace its original engine error. A success callback failure makes the operation fail.

Library `WorkflowObserver::success` runs once after a whole composed workflow succeeds; `failure` remains best-effort and preserves the original error. This is separate from per-step `after`.

## Validation Limits

Rust tests cover real source/installed commands, isolated Git remotes, simulated publication, and a loopback artifact service. They do not establish live OIDC, GitHub storage behavior, or your repository's permission/protection configuration. A successful real release is the end-to-end confirmation of that setup.
