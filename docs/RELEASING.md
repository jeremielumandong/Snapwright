# Releasing Snapwright

The release workflows are Switchyard's, adapted. Each platform is a manual workflow run on
`main` that refuses to start unless CI passed for that exact commit, then uploads to one shared
draft release `v<workspace version>`. Publish the draft by hand once all three platforms are
attached.

| Workflow | Runner | Output | Signing |
| --- | --- | --- | --- |
| `release.yml` (Linux release package) | ubuntu-22.04 | `Snapwright-<ver>-x86_64.AppImage` | none |
| `release-macos.yml` (macOS release package) | macos-15 | `Snapwright-<ver>-macos-universal.dmg` | Developer ID + notarization |
| `release-windows.yml` (Windows signed release package) | windows-2025 | `Snapwright-<ver>-windows-x64-setup.exe` | Azure Trusted Signing |
| `bump-version.yml` (Bump version) | ubuntu-latest | a PR bumping `[workspace.package] version` | none |

Each upload also gets a `.sha256` and a copy under a stable name
(`Snapwright-linux-x86_64.AppImage`, `Snapwright-macos-universal.dmg`,
`Snapwright-windows-x64-setup.exe`) for "latest" download links.

## One-time setup

### Repository settings

- Settings > Actions > General > Workflow permissions: allow "GitHub Actions to create and
  approve pull requests" (for Bump version; without it the run pushes the branch and links to
  open the PR by hand).
- Settings > Environments: create `macos-release` and `windows-release`. Add reviewers there
  if a release should need approval.

### `macos-release` environment secrets

| Name | Value |
| --- | --- |
| `MACOS_CERT_P12_BASE64` | Developer ID Application certificate and key, exported as .p12, base64 |
| `MACOS_CERT_PASSWORD` | The .p12 export password |
| `APPLE_API_KEY_ID` | App Store Connect API key ID (for notarytool) |
| `APPLE_API_ISSUER_ID` | App Store Connect issuer ID |
| `APPLE_API_KEY_P8_BASE64` | The `AuthKey_<id>.p8` file, base64 |

The signing identity is read from the certificate, so it is not a secret. With any of the five
missing the workflow still builds an ad-hoc signed DMG, keeps it as a workflow artifact, and
uploads it to the draft only when run with "Upload unsigned" ticked.

### `windows-release` environment variables (variables, not secrets)

| Name | Value |
| --- | --- |
| `AZURE_CLIENT_ID` | Entra app registration (or managed identity) client ID used for OIDC |
| `AZURE_TENANT_ID` | Entra tenant ID |
| `AZURE_SUBSCRIPTION_ID` | Subscription holding the Trusted Signing account |
| `AZURE_CODESIGNING_ENDPOINT` | e.g. `https://eus.codesigning.azure.net/` |
| `AZURE_CODESIGNING_ACCOUNT` | Trusted Signing account name |
| `AZURE_CODESIGNING_PROFILE` | Certificate profile name |
| `SNAPWRIGHT_SIGN_EXPECTED_SUBJECT` | The certificate subject, exactly as Windows shows it, without quotes (e.g. `CN=Jeremie Lumandong, O=..., C=...`) |

There are no Windows secrets: the job signs in with GitHub OIDC. The Entra app needs a federated
credential for this repository's environment, subject
`repo:jeremielumandong/Snapwright:environment:windows-release` (the case matters), and the "Trusted Signing
Certificate Profile Signer" role on the certificate profile. The Windows job fails early, before
compiling, if any variable is missing.

### Linux

Nothing to configure.

## Cutting a release

1. Run **Bump version** on `main` (patch, minor, major or an exact version) and merge its PR.
2. Wait for CI on `main` to pass for the merge commit.
3. Run the three release workflows on `main`, in any order. They share a concurrency group, so
   the first one creates the draft and the others add to it.
4. Check the draft on the Releases page and publish it.

Publishing is what ships the update: installed copies (AppImage, Windows installer, macOS app)
find the newest non-draft, non-prerelease release within a day, download the stable-named
asset (`Snapwright-linux-x86_64.AppImage`, `Snapwright-windows-x64-setup.exe`,
`Snapwright-macos-universal.dmg`), check its SHA-256 and offer to restart. Keep those names
and their `.sha256` files in `scripts/upload-release-assets.py`; renaming them strands every
installed copy.

## Building locally

```bash
packaging/package.sh                      # AppImage on Linux, DMG + PKG on macOS
packaging/linux/build-appimage.sh --skip-build
packaging/macos/build-macos.sh --arch arm64 --no-pkg
pwsh packaging/windows/build-windows.ps1  # NSIS installer, unsigned
```

Unsigned local macOS builds are ad-hoc signed. Screen Recording permission is granted per
signature, so macOS asks again after every rebuild of an ad-hoc signed app.

Icons are rendered from `packaging/icons/snapwright.svg` and `snapwright-small.svg` by
`packaging/icons/generate.py` (`pip install cairosvg pillow`); the outputs are checked in.
