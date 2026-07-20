plugins {
    id("org.jetbrains.kotlin.jvm")
    // Version resolved from settings.gradle.kts's
    // org.jetbrains.intellij.platform.settings plugin, already on the
    // classpath — declaring a version here as well conflicts with it.
    id("org.jetbrains.intellij.platform")
}

group = "ru.mellonis"
version = "0.1.0"

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        // 2024.3 (build 243) clears LSP4IJ 0.20.1's since-build=242.0 floor
        // with room to spare — the same baseline the PM-1 plugin verified
        // against the pinned LSP4IJ release.
        intellijIdeaCommunity("2024.3")
        plugin("com.redhat.devtools.lsp4ij:0.20.1")
        bundledPlugin("org.jetbrains.plugins.textmate")
    }
}

// Single-sourcing: the shared grammars ride into their bundle dirs at
// build — no second copy is ever committed. The `editors/grammars/`
// directory is the one home for both plugins' grammars, and the Rust
// drift guards in the turing-machine crate check exactly those files.
tasks.processResources {
    from("../grammars") {
        include("tmc.tmLanguage.json")
        into("textmate/tmc")
    }
    from("../grammars") {
        include("tma.tmLanguage.json")
        into("textmate/tma")
    }
}
