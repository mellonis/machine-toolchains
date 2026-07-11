package ru.mellonis.pmc

import com.intellij.openapi.application.PathManager
import org.jetbrains.plugins.textmate.api.TextMateBundleProvider
import org.jetbrains.plugins.textmate.api.TextMateBundleProvider.PluginBundle
import java.io.IOException
import java.nio.file.Files
import java.nio.file.Path

/**
 * Supplies the TextMate plugin with the `textmate/pmc` bundle built into
 * this plugin's resources: the manifest (`package.json`, committed) plus
 * the shared grammar (`pmc.tmLanguage.json`, copied in at build time from
 * `editors/grammars/` — see build.gradle.kts; never committed here).
 *
 * The TextMate plugin reads bundles off the filesystem, so both files are
 * extracted from the plugin's classloader resources to a temp directory
 * on first call.
 */
class PmcTextMateBundleProvider : TextMateBundleProvider {
    private val bundleFiles = listOf("package.json", "pmc.tmLanguage.json")

    override fun getBundles(): List<PluginBundle> {
        try {
            val tempRoot = Path.of(PathManager.getTempPath())
            Files.createDirectories(tempRoot)
            val bundleDir = Files.createTempDirectory(tempRoot, "pmc-textmate")
            for (name in bundleFiles) {
                val resource = javaClass.classLoader.getResource("textmate/pmc/$name") ?: continue
                resource.openStream().use { input ->
                    Files.copy(input, bundleDir.resolve(name))
                }
            }
            return listOf(PluginBundle("pmc", bundleDir))
        } catch (e: IOException) {
            throw RuntimeException(e)
        }
    }
}
