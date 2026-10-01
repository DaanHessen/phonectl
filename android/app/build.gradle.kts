plugins {
    id("com.android.application")
}

android {
    namespace = "com.daanh.phonectl"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.daanh.phonectl"
        minSdk = 33
        targetSdk = 36
        // Minutes since 2026-01-01: every build installs over the last one.
        versionCode = ((System.currentTimeMillis() - 1_767_225_600_000L) / 60_000L).toInt()
        versionName = "0.2.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Personal sideloaded build: sign release with the debug key so
            // `./gradlew installRelease` works without a keystore.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

dependencies {
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}
