// Lantern: the Android app, built on kit-compose (the kit's look and ease-of-use features). See README.md.
plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.baselineprofile)
}

// The version from the release workflow (the kit's kit-android.yml): -PappVersionName=0.3.1 -PappVersionCode=<code>.
// A local build without them is 0.0.1-dev, which counts every release as newer.
val appVersionName: String = providers.gradleProperty("appVersionName").orElse("0.0.1-dev").get()
val appVersionCode: Int = providers.gradleProperty("appVersionCode").orElse("1").get().toInt()

// The server suggested on first start: -PappServerUrl=… (10.0.2.2 is the computer the emulator runs on).
val appServerUrl: String = providers.gradleProperty("appServerUrl").orElse("http://10.0.2.2:9696").get()

// Release signing from the environment (CI decodes ANDROID_KEYSTORE_BASE64 to a file and sets the path).
// Without a keystore the release build is signed with the debug key; CI decides whether that is allowed (a release
// fails when any of the four secrets is missing).
val keystorePath: String? = System.getenv("ANDROID_KEYSTORE_PATH")?.takeIf { it.isNotBlank() }
val hasReleaseKey: Boolean = keystorePath?.let { file(it).isFile } == true

android {
    namespace = "dev.krifmaz.lantern"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.krifmaz.lantern"
        minSdk = 26
        targetSdk = 36
        versionCode = appVersionCode
        versionName = appVersionName

        buildConfigField("String", "DEFAULT_SERVER", "\"$appServerUrl\"")
        // where the in-app updater looks: the GitHub releases of this repository and their APK
        buildConfigField("String", "UPDATE_OWNER", "\"Krifmaz\"")
        buildConfigField("String", "UPDATE_REPO", "\"lantern\"")
        buildConfigField("String", "UPDATE_APK_PREFIX", "\"Lantern-android-\"")
        buildConfigField("String", "RELEASE_PACKAGE", "\"dev.krifmaz.lantern\"")
    }

    signingConfigs {
        create("release") {
            if (hasReleaseKey) {
                storeFile = file(keystorePath!!)
                storePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("ANDROID_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        getByName("release") {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = if (hasReleaseKey) signingConfigs.getByName("release") else signingConfigs.getByName("debug")
        }
        getByName("debug") {
            // installs beside the release build; only the release build updates itself
            applicationIdSuffix = ".debug"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }
}

dependencies {
    // the kit: theme, toasts, What's new, updates, bug reports, offline, settings (it brings Compose, OkHttp, Coil)
    implementation(libs.kit.compose)
    // installs the Baseline Profile on phones without Play's cloud profiles (and for the benchmarks)
    implementation(libs.androidx.profileinstaller)
    baselineProfile(project(":baselineprofile"))

    // JUnit 4 for app/src/test (plain Kotlin rules, no Android classes). Not in the APK.
    testImplementation(libs.junit)
}
