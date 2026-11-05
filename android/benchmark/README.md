# benchmark

Macrobenchmarks of the app on a release build (R8 on, the only place smoothness is judged):

| Test | Measures |
| --- | --- |
| `startupNoProfile` | cold start, nothing precompiled |
| `startupBaselineProfile` | cold start with the Baseline Profile (should be faster) |
| `scroll` | frame times while scrolling the home page |

```sh
./gradlew :benchmark:connectedBenchmarkReleaseAndroidTest   # a connected device or emulator, API 28+
```

Add the app's own journeys (open the main list, scroll it) to `Journeys.kt` as the app grows. CI only builds it.
