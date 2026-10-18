// The Android app's Gradle project: packages the Rust library (built by
// scripts/build-android.sh into app/src/main/jniLibs) with a small Java
// activity into an APK.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "SlipstreamRemote"
include(":app")
