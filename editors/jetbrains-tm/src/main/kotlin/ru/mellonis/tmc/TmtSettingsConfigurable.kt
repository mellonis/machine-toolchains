package ru.mellonis.tmc

import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.project.ProjectManager
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import com.redhat.devtools.lsp4ij.LanguageServerManager
import org.eclipse.lsp4j.DidChangeConfigurationParams
import javax.swing.JComponent
import javax.swing.JPanel

/** Matches `plugin.xml`'s `<server id="tmtLsp" .../>`. */
private const val TMT_LSP_SERVER_ID = "tmtLsp"

/**
 * Settings | Tools | tmt — the binary path, the lint allow-list, and the
 * opt-in warn-list. [TmtVersionCheck]'s error notification names this exact
 * location; keep both in sync if either the `groupId`/`displayName` here or
 * that message changes.
 *
 * `apply()` persists to [TmtSettings] first, then re-publishes the new lists
 * to every already-running `tmtLsp` server (one per open project) via a live
 * `workspace/didChangeConfiguration` push — see [pushConfiguration].
 */
class TmtSettingsConfigurable : Configurable {
    private val pathField = JBTextField()
    private val allowField = JBTextField()
    private val warnField = JBTextField()

    override fun getDisplayName() = "tmt"

    override fun createComponent(): JComponent {
        val state = TmtSettings.instance.state
        pathField.text = state.tmtPath
        allowField.text = state.lintAllow.joinToString(", ")
        warnField.text = state.lintWarn.joinToString(", ")
        return FormBuilder.createFormBuilder()
            .addLabeledComponent("tmt binary path:", pathField)
            .addLabeledComponent("Lint allow-list (comma-separated):", allowField)
            .addLabeledComponent("Opt-in lint rules (comma-separated):", warnField)
            .addComponentFillVertically(JPanel(), 0)
            .panel
    }

    override fun isModified(): Boolean {
        val state = TmtSettings.instance.state
        return pathField.text.trim() != state.tmtPath ||
            parseCodeList(allowField.text) != state.lintAllow ||
            parseCodeList(warnField.text) != state.lintWarn
    }

    override fun apply() {
        val state = TmtSettings.instance.state
        state.tmtPath = pathField.text.trim().ifEmpty { "tmt" }
        state.lintAllow = parseCodeList(allowField.text).toMutableList()
        state.lintWarn = parseCodeList(warnField.text).toMutableList()
        pushConfiguration(state.lintAllow, state.lintWarn)
    }

    override fun reset() {
        val state = TmtSettings.instance.state
        pathField.text = state.tmtPath
        allowField.text = state.lintAllow.joinToString(", ")
        warnField.text = state.lintWarn.joinToString(", ")
    }

    private fun parseCodeList(raw: String): List<String> =
        raw.split(",").map(String::trim).filter(String::isNotEmpty)

    /**
     * Pushes `{"tmt": {"lint": {"allow": [...], "warn": [...]}}}` (the same
     * wrapped shape the Rust server unwraps via its `"tmt"` key) to every open
     * project's `tmtLsp` server.
     *
     * Mechanism: LSP4IJ 0.20.1's `LanguageServerItem.getWorkspaceService()`
     * exposes the raw LSP4J `WorkspaceService` proxy for an already-running
     * server, so `didChangeConfiguration` goes straight over the wire — no
     * restart needed, and the server re-lints every open document after a
     * config change. `LanguageServerManager.getLanguageServer(id)` resolves
     * only servers that are ALREADY started, so a project with no running
     * tmtLsp server is silently skipped — its next `initialize` reads the
     * persisted settings fresh, which is already correct.
     */
    private fun pushConfiguration(lintAllow: List<String>, lintWarn: List<String>) {
        val settings = mapOf("tmt" to mapOf("lint" to mapOf("allow" to lintAllow, "warn" to lintWarn)))
        for (project in ProjectManager.getInstance().openProjects) {
            LanguageServerManager.getInstance(project)
                .getLanguageServer(TMT_LSP_SERVER_ID)
                .thenAccept { item ->
                    item?.workspaceService?.didChangeConfiguration(DidChangeConfigurationParams(settings))
                }
                .exceptionally {
                    LOG.warn("tmt lsp configuration push failed", it)
                    null
                }
        }
    }

    private companion object {
        val LOG = Logger.getInstance(TmtSettingsConfigurable::class.java)
    }
}
