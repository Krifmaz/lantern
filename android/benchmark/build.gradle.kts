// Macrobenchmarks of the app (startup, scrolling) on a connected device or emulator:
//   ./gradlew :benchmark:connectedBenchmarkReleaseAndroidTest
// Not run in CI (no emulator there); CI only builds it. See README.md.
plugins {
    alias(libs.plugins.android.test)
    alias(libs.plugins.baselineprofile)
}

android {
    namespace = "dev.krifmaz.lantern.benchmark"
    compileSdk = 36

    defaultConfig {
        // Macrobenchmark needs API 28 (Baseline Profiles are collected from API 28 on, rooted or not from 33)
        minSdk = 28
        targetSdk = 36
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    targetProjectPath = ":app"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

// a connected device or emulator ("gradle-managed devices" need an image download, which is up to the app)
baselineProfile {
    useConnectedDevices = true
}

dependencies {
    implementation(libs.androidx.test.ext.junit)
    implementation(libs.androidx.test.uiautomator)
    implementation(libs.androidx.benchmark.macro.junit4)
}
