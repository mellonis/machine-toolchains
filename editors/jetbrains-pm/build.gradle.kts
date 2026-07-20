plugins {
    id("org.jetbrains.kotlin.jvm")
    // Version resolved from settings.gradle.kts's
    // org.jetbrains.intellij.platform.settings plugin, already on the
    // classpath — declaring a version here as well conflicts with it.
    id("org.jetbrains.intellij.platform")
}

group = "ru.mellonis"
version = "0.1.2"

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        // 2024.3 (build 243) clears LSP4IJ 0.20.1's since-build=242.0 floor
        // with room to spare — the lowest Community baseline verified
        // against the pinned LSP4IJ release at implementation time.
        intellijIdeaCommunity("2024.3")
        // Pinned to the latest stable release on the JetBrains Marketplace
        // as of implementation (verified via the plugin's updates API).
        plugin("com.redhat.devtools.lsp4ij:0.20.1")
        bundledPlugin("org.jetbrains.plugins.textmate")
    }
}

// Single-sourcing: the shared grammars ride into their bundle dirs at
// build — no second copy is ever committed. See
// docs/superpowers/plans/2026-07-10-lsp-plan3-editor-shells.md.
tasks.processResources {
    from("../grammars") {
        include("pmc.tmLanguage.json")
        into("textmate/pmc")
    }
    from("../grammars") {
        include("pma.tmLanguage.json")
        into("textmate/pma")
    }
}
