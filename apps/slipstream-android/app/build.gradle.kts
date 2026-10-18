plugins {
    id("com.android.application")
}

// Set by scripts/build-android.sh from Cargo.toml and the git commit count,
// like the desktop apps' version and build number.
val slipstreamVersion = providers.gradleProperty("slipstreamVersion").getOrElse("0.0.0")
val slipstreamBuild = providers.gradleProperty("slipstreamBuild").getOrElse("1").toInt()

android {
    namespace = "com.slipstreamremote.app"
    compileSdk = 36
    ndkVersion = "29.0.14206865"

    defaultConfig {
        applicationId = "com.slipstreamremote.app"
        // Android 8: AAudio, which remote sound plays through.
        minSdk = 26
        targetSdk = 36
        versionCode = slipstreamBuild
        versionName = slipstreamVersion
    }

    buildTypes {
        release {
            // Shrinks AppCompat's Java code (most of the APK's classes.dex).
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Until the release key is set up (docs/ANDROID.md), release
            // builds are signed with this computer's debug key.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation("androidx.appcompat:appcompat:1.7.1")
    // The version android-activity 0.6 is built against (its README).
    implementation("androidx.games:games-activity:4.4.0")
}
