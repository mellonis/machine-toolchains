package ru.mellonis.pmc

import com.intellij.openapi.application.PathManager
import com.intellij.openapi.diagnostic.Logger
import org.jetbrains.plugins.textmate.api.TextMateBundleProvider
import org.jetbrains.plugins.textmate.api.TextMateBundleProvider.PluginBundle
import java.io.IOException
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption

/**
 * Supplies the TextMate plugin with the `textmate/pmc` bundle built into
 * this plugin's resources: the manifest (`package.json`, committed) plus
 * the shared grammar (`pmc.tmLanguage.json`, copied in at build time from
 * `editors/grammars/` — see build.gradle.kts; never committed here).
 *
 * The TextMate plugin reads bundles off the filesystem, so both files are
 * extracted from the plugin's classloader resources into a **stable**
 * named directory under the IDE temp path, overwritten on every call —
 * `getBundles()` can be invoked more than once per IDE session (e.g. on
 * plugin reload), and a fresh `Files.createTempDirectory` name each time
 * would leak one directory per call instead of reusing one.
 */
class PmcTextMateBundleProvider : TextMateBundleProvider {
    private val bundleFiles = listOf("package.json", "pmc.tmLanguage.json")

    override fun getBundles(): List<PluginBundle> {
        try {
            val bundleDir = Path.of(PathManager.getTempPath(), "pmc-textmate")
            Files.createDirectories(bundleDir)
            for (name in bundleFiles) {
                val resource = javaClass.classLoader.getResource("textmate/pmc/$name")
                if (resource == null) {
                    // A bundled resource missing at runtime means the
                    // plugin jar itself is broken (build.gradle.kts's
                    // processResources copy step failed or the resource
                    // was never packaged) — fail loudly rather than
                    // silently shipping a partial/uncolored bundle.
                    val message = "pmc TextMate bundle resource missing: textmate/pmc/$name (broken plugin packaging)"
                    LOG.error(message)
                    throw IllegalStateException(message)
                }
                resource.openStream().use { input ->
                    Files.copy(input, bundleDir.resolve(name), StandardCopyOption.REPLACE_EXISTING)
                }
            }
            return listOf(PluginBundle("pmc", bundleDir))
        } catch (e: IOException) {
            throw RuntimeException(e)
        }
    }

    private companion object {
        val LOG = Logger.getInstance(PmcTextMateBundleProvider::class.java)
    }
}
