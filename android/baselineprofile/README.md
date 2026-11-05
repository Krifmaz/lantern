# baselineprofile

Generates the app's Baseline Profile and startup profile (`androidx.baselineprofile`): a cold start, Continue on the
first-start page when it shows, then scrolling the home page (`BaselineProfileGenerator`, journeys in `Journeys.kt`).
The profile lands in `app/src/release/generated/` and ships in the release APK; `profileinstaller` installs it.
Commit it after generating.

```sh
./gradlew :app:generateBaselineProfile   # a connected device or emulator, API 28+, with the server running
                                         # and the app signed in once by hand
```

CI only builds this module (no emulator). Every release APK carries `assets/dexopt/baseline.prof` anyway (the
libraries' own profiles); the generated one adds the app's journeys.
