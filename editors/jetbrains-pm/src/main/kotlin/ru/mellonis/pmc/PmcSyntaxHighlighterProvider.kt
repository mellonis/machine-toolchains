package ru.mellonis.pmc

import com.intellij.openapi.fileTypes.FileType
import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.fileTypes.SyntaxHighlighterProvider
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import org.jetbrains.plugins.textmate.language.syntax.highlighting.TextMateSyntaxHighlighterFactory

/**
 * The file-TYPE-keyed syntax highlighter for PMC/PMA (the
 * `syntaxHighlighter` extension, keyed by file-type name). This is the
 * lookup newer platforms consult for a non-language file type when
 * building an editor highlighter — the `editorHighlighterProvider`
 * extension is bypassed on that path — so without this registration the
 * editor silently falls back to plain text. The TextMate factory resolves
 * the grammar by file NAME against the registered bundles, which is what
 * makes the answer land on ours.
 */
class PmcSyntaxHighlighterProvider : SyntaxHighlighterProvider {
    override fun create(fileType: FileType, project: Project?, file: VirtualFile?): SyntaxHighlighter? =
        TextMateSyntaxHighlighterFactory().getSyntaxHighlighter(project, file)
}
