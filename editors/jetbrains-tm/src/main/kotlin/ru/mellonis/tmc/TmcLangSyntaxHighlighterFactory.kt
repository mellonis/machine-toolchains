package ru.mellonis.tmc

import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.fileTypes.SyntaxHighlighterFactory
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import org.jetbrains.plugins.textmate.language.syntax.highlighting.TextMateSyntaxHighlighterFactory

/**
 * Language-keyed twin of [TmcSyntaxHighlighterProvider], serving the
 * `SyntaxHighlighterFactory.getSyntaxHighlighter(Language, ...)` lookups
 * (code fragments, previews) for [TmcLanguage]/[TmaLanguage] — the same
 * file-NAME-based TextMate resolution either way.
 */
class TmcLangSyntaxHighlighterFactory : SyntaxHighlighterFactory() {
    override fun getSyntaxHighlighter(project: Project?, virtualFile: VirtualFile?): SyntaxHighlighter =
        TextMateSyntaxHighlighterFactory().getSyntaxHighlighter(project, virtualFile)
}
