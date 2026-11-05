# Lantern for Android

The phone app: Jetpack Compose on [kit-compose](https://github.com/Krifmaz/kit/tree/v0.2.1/android/kit-compose)
(v0.2.1), in the kit's default theme. A Gradle build of its own; the rest of the repository is Rust.

| Module | What it is |
| --- | --- |
| [`app/`](app/README.md) | The app (`dev.krifmaz.lantern`): connect to the server, then the kit's features against it |
| [`baselineprofile/`](baselineprofile/README.md) | Generates the app's Baseline Profile and startup profile |
| [`benchmark/`](benchmark/README.md) | Macrobenchmarks of the release build: cold start and scrolling |

## Build

kit-compose comes from the kit's GitHub Packages, which asks for a sign-in even to read: put a GitHub user and a
token with `read:packages` in `~/.gradle/gradle.properties` (never in this repository):

```properties
gpr.user=<your GitHub user>
gpr.key=<a token with read:packages>
```

(`GITHUB_ACTOR` / `GITHUB_TOKEN` in the environment work too.) Then, with the Android SDK installed:

```sh
cd android
./gradlew :app:assembleDebug        # build
./gradlew :app:installDebug         # on a connected phone or emulator
./gradlew test                      # unit tests
```

| Gradle property | Meaning | Default |
| --- | --- | --- |
| `appVersionName`, `appVersionCode` | the version (the release workflow passes them) | `0.0.1-dev`, `1` |
| `appServerUrl` | the server suggested on first start | `http://10.0.2.2:9696` (the computer the emulator runs on) |
| `kitPath` (or `KIT_PATH`) | build kit-compose from a kit checkout (`<kitPath>/android`) instead of the package | — |
| `gpr.user`, `gpr.key` | sign-in for the kit's GitHub Packages | `GITHUB_ACTOR`, `GITHUB_TOKEN` |

## Releases

The release workflow builds the signed APK `Lantern-android-<version>.apk` with the kit's `kit-android.yml`
(Actions → release → Run workflow with "apps" ticked). It needs four repository secrets, the permanent release key:
`ANDROID_KEYSTORE_BASE64` (`base64 -w0 release.jks`), `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` and
`ANDROID_KEY_PASSWORD`; Gradle reads them as `ANDROID_KEYSTORE_PATH`, `ANDROID_KEYSTORE_PASSWORD`,
`ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`. Without them a local release build is signed with the debug key. The
installed app finds the APK on this repository's releases and updates itself.

## Checks

ktlint (`.editorconfig`) and `./gradlew test` pass before every commit. Judge smoothness only on a release build
(`benchmark/`), and regenerate the Baseline Profile after big changes to the first screens.
