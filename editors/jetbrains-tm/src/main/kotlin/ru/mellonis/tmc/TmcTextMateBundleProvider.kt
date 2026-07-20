package ru.mellonis.tmc

import com.intellij.openapi.application.PathManager
import com.intellij.openapi.diagnostic.Logger
import org.jetbrains.plugins.textmate.api.TextMateBundleProvider
import org.jetbrains.plugins.textmate.api.TextMateBundleProvider.PluginBundle
import java.io.IOException
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption

/**
 * Supplies the TextMate plugin with the `textmate/tmc` and `textmate/tma`
 * bundles built into this plugin's resources: each bundle's manifest
 * (`package.json`, committed) plus its shared grammar (`*.tmLanguage.json`,
 * copied in at build time from `editors/grammars/` — see
 * build.gradle.kts; never committed here).
 *
 * `TextMateBundleProvider.getBundles()` already returns a `List`, so one
 * provider instance serving both bundles is the natural shape — no need
 * for a second provider class. The TextMate plugin reads bundles off the
 * filesystem, so each bundle's files are extracted from the plugin's
 * classloader resources into a **stable** named directory under the IDE
 * temp path, overwritten on every call — `getBundles()` can be invoked
 * more than once per IDE session (e.g. on plugin reload), and a fresh
 * `Files.createTempDirectory` name each time would leak one directory per
 * call instead of reusing one.
 *
 * The directory names carry a `tm-` prefix so a machine with BOTH this
 * plugin and the PM-1 one installed never has the two providers writing
 * into the same IDE-temp bundle directory.
 */
class TmcTextMateBundleProvider : TextMateBundleProvider {
    private data class Bundle(val name: String, val files: List<String>)

    private val bundles = listOf(
        Bundle("tmc", listOf("package.json", "tmc.tmLanguage.json")),
        Bundle("tma", listOf("package.json", "tma.tmLanguage.json")),
    )

    override fun getBundles(): List<PluginBundle> {
        try {
            return bundles.map { bundle ->
                val bundleDir = Path.of(PathManager.getTempPath(), "tm-${bundle.name}-textmate")
                Files.createDirectories(bundleDir)
                for (name in bundle.files) {
                    val resource = javaClass.classLoader.getResource("textmate/${bundle.name}/$name")
                    if (resource == null) {
                        // A bundled resource missing at runtime means the
                        // plugin jar itself is broken (build.gradle.kts's
                        // processResources copy step failed or the resource
                        // was never packaged) — fail loudly rather than
                        // silently shipping a partial/uncolored bundle.
                        val message =
                            "${bundle.name} TextMate bundle resource missing: textmate/${bundle.name}/$name (broken plugin packaging)"
                        LOG.error(message)
                        throw IllegalStateException(message)
                    }
                    resource.openStream().use { input ->
                        Files.copy(input, bundleDir.resolve(name), StandardCopyOption.REPLACE_EXISTING)
                    }
                }
                PluginBundle(bundle.name, bundleDir)
            }
        } catch (e: IOException) {
            throw RuntimeException(e)
        }
    }

    private companion object {
        val LOG = Logger.getInstance(TmcTextMateBundleProvider::class.java)
    }
}
