// Lantern for Android: the root build. The modules are app/ (the app), baselineprofile/ (its Baseline Profile)
// and benchmark/ (its Macrobenchmarks).
//
// AGP 9 compiles Kotlin itself ("built-in Kotlin"), so org.jetbrains.kotlin.android is not applied anywhere.
// The classpath entry raises the Kotlin Gradle plugin AGP brings to the Kotlin pinned in gradle/libs.versions.toml
// (the Compose compiler plugin must match it exactly). The versions are kit-compose's (the kit's
// android/gradle/libs.versions.toml), so the kit, built in with -PkitPath, brings the same AGP and Kotlin.
buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.4.21")
    }
}

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.test) apply false
    alias(libs.plugins.baselineprofile) apply false
    alias(libs.plugins.kotlin.compose) apply false
}
