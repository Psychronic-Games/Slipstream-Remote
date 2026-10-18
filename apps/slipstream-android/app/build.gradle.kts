plugins {
    id("com.android.application")
}

// Set by scripts/build-android.sh from Cargo.toml and the git commit count,
// like the desktop apps' version and build number.
val slipstreamVersion = providers.gradleProperty("slipstreamVersion").getOrElse("0.0.0")
val slipstreamBuild = providers.gradleProperty("slipstreamBuild").getOrElse("1").toInt()
// The release key (scripts/build-android.sh passes it). Android installs an
// update only if it's signed with the same key as the installed app.
val releaseKeystore = providers.environmentVariable("SLIPSTREAM_ANDROID_KEYSTORE").orNull

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

    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                storeFile = file(releaseKeystore)
                storeType = "pkcs12"
                storePassword = providers.environmentVariable("SLIPSTREAM_ANDROID_KEYSTORE_PASSWORD").get()
                keyAlias = "slipstream"
                keyPassword = storePassword
            }
        }
    }

    buildTypes {
        release {
            // Shrinks AppCompat's Java code (most of the APK's classes.dex).
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Without the release key (a plain Gradle build), this
            // computer's debug key.
            signingConfig = signingConfigs.getByName(if (releaseKeystore != null) "release" else "debug")
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
