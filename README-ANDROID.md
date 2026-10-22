# Concat for Android — separated build tree

This is the Android build tree of Concat (the free, open-source CapCut
replacement), separated from the main repository so the Android app is
easy to find and build on its own. No code has been changed.

## Where the Android app lives

- `src/crates/concat-android/` — the Android app itself: the Rust entry
  point (`src/lib.rs`), the Java bridge (`java/ConcatFiles.java`), the
  launcher icons (`res/`) and the manifest metadata (in its Cargo.toml).
- Everything else under `src/crates/` is the shared editor engine the
  Android app is built from — the Android crate embeds the full `concat`
  app, so the engine has to travel with it for the build to work.

## Build the APK with GitHub Actions

1. Push this folder's contents to a GitHub repository.
2. Open the Actions tab and run the `mobile` workflow (it also runs
   automatically on pushes to `main`).
3. Download the `Concat-android-arm64` artifact from the finished run —
   inside is the installable APK.
4. Optional, for signed releases: create the `ANDROID_KEYSTORE` and
   `ANDROID_KEYSTORE_PASSWORD` repository secrets (see CODE_SIGNING.md),
   then push a version tag (e.g. `v0.2.8`) — the `release` workflow
   builds, signs and drafts a GitHub release.

Licence: AGPL-3.0 — see LICENSE and THIRD_PARTY_NOTICES.md.
