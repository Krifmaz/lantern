// Lantern for Android: a Gradle build of its own (the rest of the repository is Rust). See README.md.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

// kit-compose comes from the kit's GitHub Packages (DESIGN.md §15). GitHub asks for a sign-in even to read it: a
// GitHub user and a token with read:packages, as gpr.user / gpr.key in ~/.gradle/gradle.properties (never in this
// repository), or GITHUB_ACTOR / GITHUB_TOKEN in the environment (GitHub Actions).
val gprUser: String? = providers.gradleProperty("gpr.user").orElse(providers.environmentVariable("GITHUB_ACTOR")).orNull
val gprKey: String? = providers.gradleProperty("gpr.key").orElse(providers.environmentVariable("GITHUB_TOKEN")).orNull

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.PREFER_SETTINGS)
    repositories {
        google()
        mavenCentral()
        maven {
            name = "KitPackages"
            url = uri("https://maven.pkg.github.com/Krifmaz/kit")
            if (gprUser != null && gprKey != null) {
                credentials {
                    username = gprUser
                    password = gprKey
                }
            }
            // only the kit is looked for there
            content { includeGroup("dev.krifmaz.kit") }
        }
    }
}

// The kit from source instead: -PkitPath=<a checkout of Krifmaz/kit> (or KIT_PATH in the environment) builds
// kit-compose from <kit>/android, and Gradle uses it for dev.krifmaz.kit:kit-compose. The kit's own CI builds this
// app that way against a pull request's kit-compose; use it to try a kit change before it is released.
val kitPath: String? = providers.gradleProperty("kitPath").orElse(providers.environmentVariable("KIT_PATH")).orNull?.takeIf { it.isNotBlank() }
if (kitPath != null) {
    includeBuild("$kitPath/android")
}

rootProject.name = "lantern-android"
include(":app")
// the Baseline Profile generator and the Macrobenchmarks of :app (they need a device or emulator to run)
include(":baselineprofile")
include(":benchmark")
