package ru.mellonis.tmc

import com.intellij.openapi.editor.colors.EditorColorsScheme
import com.intellij.openapi.editor.ex.util.DataStorage
import com.intellij.openapi.editor.ex.util.LexerEditorHighlighter
import com.intellij.openapi.editor.highlighter.EditorHighlighter
import com.intellij.openapi.fileTypes.EditorHighlighterProvider
import com.intellij.openapi.fileTypes.FileType
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import org.jetbrains.plugins.textmate.language.syntax.highlighting.TextMateSyntaxHighlighterFactory
import org.jetbrains.plugins.textmate.language.syntax.lexer.TextMateLexerDataStorage

/**
 * Colors TMC/TMA editors from the bundled TextMate grammars.
 *
 * Not the TextMate plugin's own `EditorHighlighterProvider`: that class
 * resolves its highlighter through the platform's file-TYPE-keyed lookup,
 * which knows nothing about this plugin's file types and quietly falls
 * back to plain text. `TextMateSyntaxHighlighterFactory`'s file-NAME
 * lookup is the one that consults the registered bundles, so this
 * provider calls it directly and wraps the result the way the TextMate
 * plugin's own (private) editor highlighter does — including the
 * token-data storage its lexer's rich token types require.
 */
class TmcTextMateEditorHighlighterProvider : EditorHighlighterProvider {
    override fun getEditorHighlighter(
        project: Project?,
        fileType: FileType,
        virtualFile: VirtualFile?,
        colors: EditorColorsScheme,
    ): EditorHighlighter {
        val highlighter = TextMateSyntaxHighlighterFactory().getSyntaxHighlighter(project, virtualFile)
        return object : LexerEditorHighlighter(highlighter, colors) {
            override fun createStorage(): DataStorage = TextMateLexerDataStorage()
        }
    }
}
