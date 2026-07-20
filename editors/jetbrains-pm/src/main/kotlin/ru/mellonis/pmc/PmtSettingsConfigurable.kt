package ru.mellonis.pmc

import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.project.ProjectManager
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import com.redhat.devtools.lsp4ij.LanguageServerManager
import org.eclipse.lsp4j.DidChangeConfigurationParams
import javax.swing.JComponent
import javax.swing.JPanel

/** Matches `plugin.xml`'s `<server id="pmtLsp" .../>`. */
private const val PMT_LSP_SERVER_ID = "pmtLsp"

/**
 * Settings | Tools | pmt — the binary path + lint allow-list.
 * [PmtVersionCheck]'s error notification names this exact location; keep
 * both in sync if either the `groupId`/`displayName` here or that message
 * changes.
 *
 * `apply()` persists to [PmtSettings] first, then re-publishes the new
 * allow-list to every already-running `pmtLsp` server (one per open
 * project) via a live `workspace/didChangeConfiguration` push — see
 * [pushConfiguration] for the mechanism and why no restart is needed.
 */
class PmtSettingsConfigurable : Configurable {
    private val pathField = JBTextField()
    private val allowField = JBTextField()

    override fun getDisplayName() = "pmt"

    override fun createComponent(): JComponent {
        val state = PmtSettings.instance.state
        pathField.text = state.pmtPath
        allowField.text = state.lintAllow.joinToString(", ")
        return FormBuilder.createFormBuilder()
            .addLabeledComponent("pmt binary path:", pathField)
            .addLabeledComponent("Lint allow-list (comma-separated):", allowField)
            .addComponentFillVertically(JPanel(), 0)
            .panel
    }

    override fun isModified(): Boolean {
        val state = PmtSettings.instance.state
        return pathField.text.trim() != state.pmtPath || parseAllowList(allowField.text) != state.lintAllow
    }

    override fun apply() {
        val state = PmtSettings.instance.state
        state.pmtPath = pathField.text.trim().ifEmpty { "pmt" }
        state.lintAllow = parseAllowList(allowField.text).toMutableList()
        pushConfiguration(state.lintAllow)
    }

    override fun reset() {
        val state = PmtSettings.instance.state
        pathField.text = state.pmtPath
        allowField.text = state.lintAllow.joinToString(", ")
    }

    private fun parseAllowList(raw: String): List<String> = raw.split(",").map(String::trim).filter(String::isNotEmpty)

    /**
     * Pushes `{"pmt": {"lint": {"allow": [...]}}}` (the same wrapped shape
     * the Rust server unwraps via its `"pmt"` key — see
     * `crates/post-machine/src/lsp/mod.rs::did_change_config`) to every
     * open project's `pmtLsp` server.
     *
     * Mechanism: LSP4IJ 0.20.1's `LanguageServerItem.getWorkspaceService()`
     * exposes the raw LSP4J `WorkspaceService` proxy for an already-running
     * server, so `didChangeConfiguration` goes straight over the wire — no
     * restart needed, and the server's own handler already republishes
     * diagnostics for every open document after a config change (see
     * `handle_notification`'s `"workspace/didChangeConfiguration"` arm).
     * `LanguageServerManager.getLanguageServer(id)` resolves only servers
     * that are ALREADY started (it does not start one to answer the
     * query — confirmed by decompiling 0.20.1: it delegates to
     * `LanguageServiceAccessor.getLanguageServers` over the started-servers
     * set), so a project with no running pmtLsp server is silently
     * skipped — its next `initialize` reads the persisted settings fresh,
     * which is already correct. This is why no `LanguageServerManager`
     * stop/start fallback is needed here: the notification hook exists.
     */
    private fun pushConfiguration(lintAllow: List<String>) {
        val settings = mapOf("pmt" to mapOf("lint" to mapOf("allow" to lintAllow)))
        for (project in ProjectManager.getInstance().openProjects) {
            LanguageServerManager.getInstance(project)
                .getLanguageServer(PMT_LSP_SERVER_ID)
                .thenAccept { item ->
                    item?.workspaceService?.didChangeConfiguration(DidChangeConfigurationParams(settings))
                }
                .exceptionally {
                    LOG.warn("pmt lsp configuration push failed", it)
                    null
                }
        }
    }

    private companion object {
        val LOG = Logger.getInstance(PmtSettingsConfigurable::class.java)
    }
}
