package ru.mellonis.pmc

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.redhat.devtools.lsp4ij.LanguageServerFactory
import com.redhat.devtools.lsp4ij.server.ProcessStreamConnectionProvider
import com.redhat.devtools.lsp4ij.server.StreamConnectionProvider

class PmtLanguageServerFactory : LanguageServerFactory {
    override fun createConnectionProvider(project: Project): StreamConnectionProvider =
        PmtConnectionProvider()
}

/**
 * Launches `pmt lsp` on stdio and forwards the lint allow-list as LSP
 * initializationOptions — the shape the server expects
 * (`{"lint":{"allow":[...]}}`). Both the binary path and the allow-list
 * come from [PmtSettings], live at connect time.
 */
class PmtConnectionProvider :
    ProcessStreamConnectionProvider(listOf(PmtSettings.instance.state.pmtPath, "lsp")) {
    override fun getInitializationOptions(rootUri: VirtualFile?): Any =
        mapOf("lint" to mapOf("allow" to PmtSettings.instance.state.lintAllow))
}
