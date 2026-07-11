// Java environment: no system JDK is assumed. Point JAVA_HOME at any JDK
// before running ./gradlew — e.g. the JetBrains Runtime bundled with a
// Toolbox IDE:
//   export JAVA_HOME="$HOME/Applications/<SomeIDE>.app/Contents/jbr/Contents/Home"
// This is machine-specific and deliberately not committed (see
// gradle.properties and README.md). The foojay-resolver-convention plugin
// below lets Gradle auto-provision the toolchain the IntelliJ Platform
// Gradle Plugin needs for compilation if the JAVA_HOME JDK can't serve it.
import org.jetbrains.intellij.platform.gradle.extensions.intellijPlatform

pluginManagement {
    plugins {
        id("org.jetbrains.kotlin.jvm") version "2.1.21"
    }
}

plugins {
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
    id("org.jetbrains.intellij.platform.settings") version "2.18.1"
}

rootProject.name = "pmc"

@Suppress("UnstableApiUsage")
dependencyResolutionManagement {
    repositories {
        mavenCentral()
        intellijPlatform {
            defaultRepositories()
        }
    }
}
