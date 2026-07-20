package ru.mellonis.tmc

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.redhat.devtools.lsp4ij.LanguageServerFactory
import com.redhat.devtools.lsp4ij.server.ProcessStreamConnectionProvider
import com.redhat.devtools.lsp4ij.server.StreamConnectionProvider

class TmtLanguageServerFactory : LanguageServerFactory {
    override fun createConnectionProvider(project: Project): StreamConnectionProvider =
        TmtConnectionProvider()
}

/**
 * Launches `tmt lsp` on stdio and forwards the lint allow/warn lists as LSP
 * initializationOptions — the shape the server expects
 * (`{"lint":{"allow":[...],"warn":[...]}}`). One process serves BOTH `.tmc`
 * and `.tma`; the server routes each document to its own language service by
 * extension. Both the binary path and the lists come from [TmtSettings],
 * live at connect time.
 */
class TmtConnectionProvider :
    ProcessStreamConnectionProvider(listOf(TmtSettings.instance.state.tmtPath, "lsp")) {
    override fun getInitializationOptions(rootUri: VirtualFile?): Any =
        mapOf(
            "lint" to mapOf(
                "allow" to TmtSettings.instance.state.lintAllow,
                "warn" to TmtSettings.instance.state.lintWarn,
            ),
        )
}
