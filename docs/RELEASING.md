<!-- SPDX-FileCopyrightText: 2026 Hakoniwa -->
<!-- SPDX-License-Identifier: MPL-2.0 -->

# Releasing Efude

Releases are built and published by the `CI` workflow
(`.github/workflows/ci.yml`) when a version tag (`v*`) is pushed.

## Steps

1. Set the version in the root `Cargo.toml` (`[workspace.package] version`,
   three numbers such as `0.1.0`; the MSI installer needs this form).
2. Commit and push, and wait until the `CI` run for that commit is green.
3. Create and push the tag, for example `v0.1.0`:

   ```sh
   git tag -a v0.1.0 -m "Efude v0.1.0"
   git push origin v0.1.0
   ```

   The maintainer's local `release.bat` (not in the repository) does the same
   after checking that everything is committed and pushed.
4. The tag's CI run builds on Windows, then the `release` job publishes a
   GitHub Release named `Efude v0.1.0` with:
   - `Efude-v0.1.0-windows-x64.zip` — portable build (`efude.exe`, license
     files, third-party notices and the user guide),
   - `Efude-v0.1.0-windows-x64.msi` — installer,
   - `THIRD_PARTY_NOTICES.txt`.

   Release notes are generated from the commits since the previous tag; edit
   them on the release page if needed.

Tags with a suffix (`v0.2.0-beta.1`) are published as pre-releases. A failed
release can be retried by deleting the release and the tag on GitHub, fixing
the problem, and pushing the tag again.

## Publishing the engine crates to crates.io

The engine crates (`efude-core`, `efude-input`, `efude-stroke`,
`efude-brush`, `efude-canvas`, `efude-gpu`, `efude-io`, `efude-comic`) and
the `efude` crate that re-exports them are published under MIT OR Apache-2.0.
The UI and the app are not (`publish = false`).

1. Sign in at <https://crates.io> with the GitHub account and create an API
   token (Account Settings → API Tokens, scope `publish-new` and
   `publish-update`), then run `cargo login` and paste it when asked.
2. Check, then publish every crate in dependency order:

   ```sh
   cargo publish --workspace --dry-run
   cargo publish --workspace
   ```

A published version cannot be replaced (only yanked), so publish after the
release tag's CI run is green.

## Windows release signing

The Windows CI workflow can sign the release executable and MSI before it packages the portable ZIP. Signing runs only for version tags (`v*`) and only when repository variable `WINDOWS_SIGNING_ENABLED` is set to `true`.

Configure these repository Actions secrets:

- `WINDOWS_SIGNING_CERTIFICATE_PFX_B64`: the base64-encoded code-signing PFX certificate.
- `WINDOWS_SIGNING_CERTIFICATE_PASSWORD`: the PFX password.

The certificate must be valid for code signing and trusted by Windows. The workflow uses SHA-256 file digests, RFC 3161 timestamps from DigiCert, and removes the temporary PFX after signing. Keep signing secrets unavailable to pull request builds. If signing is not enabled, CI still produces unsigned development artifacts; do not publish them as signed releases.
