package ru.mellonis.pmc

import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.fileTypes.SyntaxHighlighterFactory
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import org.jetbrains.plugins.textmate.language.syntax.highlighting.TextMateSyntaxHighlighterFactory

/**
 * Language-keyed twin of [PmcSyntaxHighlighterProvider], serving the
 * `SyntaxHighlighterFactory.getSyntaxHighlighter(Language, ...)` lookups
 * (code fragments, previews) for [PmcLanguage]/[PmaLanguage] — the same
 * file-NAME-based TextMate resolution either way.
 */
class PmcLangSyntaxHighlighterFactory : SyntaxHighlighterFactory() {
    override fun getSyntaxHighlighter(project: Project?, virtualFile: VirtualFile?): SyntaxHighlighter =
        TextMateSyntaxHighlighterFactory().getSyntaxHighlighter(project, virtualFile)
}
