package ru.mellonis.pmc

import com.intellij.extapi.psi.ASTWrapperPsiElement
import com.intellij.extapi.psi.PsiFileBase
import com.intellij.lang.ASTNode
import com.intellij.lang.Language
import com.intellij.lang.ParserDefinition
import com.intellij.lang.PsiBuilder
import com.intellij.lang.PsiParser
import com.intellij.lexer.LexerBase
import com.intellij.openapi.fileTypes.FileType
import com.intellij.openapi.project.Project
import com.intellij.psi.FileViewProvider
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import com.intellij.psi.tree.IFileElementType
import com.intellij.psi.tree.TokenSet

/**
 * The smallest parser definition a language can have: a flat run of
 * word / whitespace / other tokens under one file node. It exists so the
 * platform builds a PSI file that really belongs to
 * [PmcLanguage]/[PmaLanguage] instead of falling back to a plain-text
 * PSI file whose language disagrees with the file's base language. That
 * disagreement is what silently kept LSP4IJ from connecting an opened
 * document to the server — no `didOpen`, hence no navigation, no hover —
 * once the file types became language file types for the sake of
 * coloring. Tokens are word-grained rather than one whole-file token
 * because the platform underlines the PSI element under the caret on
 * Cmd/Ctrl+hover: with one token the whole document lit up, with word
 * tokens only the identifier does. All real language intelligence stays
 * with `pmt lsp`; nothing here is ever read for semantics.
 */
abstract class TextOnlyParserDefinition(
    private val ownLanguage: Language,
    private val ownFileType: FileType,
) : ParserDefinition {
    private val fileNode = IFileElementType(ownLanguage)
    private val wordToken = IElementType("WORD", ownLanguage)
    private val textToken = IElementType("TEXT", ownLanguage)

    override fun createLexer(project: Project?) = WordLexer(wordToken, textToken)
    override fun createParser(project: Project?) = WholeTextParser()
    override fun getFileNodeType(): IFileElementType = fileNode
    override fun getCommentTokens(): TokenSet = TokenSet.EMPTY
    override fun getStringLiteralElements(): TokenSet = TokenSet.EMPTY
    override fun createElement(node: ASTNode): PsiElement = ASTWrapperPsiElement(node)
    override fun createFile(viewProvider: FileViewProvider): PsiFile =
        // The field names are deliberately NOT `fileType`/`language`: inside
        // this object expression a bare `fileType` resolves to the object's
        // own synthetic property (its `getFileType()`), not the outer field,
        // and the override would call itself forever.
        object : PsiFileBase(viewProvider, ownLanguage) {
            override fun getFileType(): FileType = this@TextOnlyParserDefinition.ownFileType
        }
}

/**
 * Splits the buffer into maximal runs of three classes: word characters
 * (letters, digits, `_`) as [word], whitespace as the platform's own
 * whitespace token, and everything else as [other]. Total: every
 * character lands in exactly one token, so the tree stays lossless.
 */
class WordLexer(private val word: IElementType, private val other: IElementType) : LexerBase() {
    private var buffer: CharSequence = ""
    private var end = 0
    private var tokenStart = 0
    private var tokenEnd = 0

    override fun start(buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer
        end = endOffset
        tokenStart = startOffset
        tokenEnd = startOffset
        scan()
    }

    private fun classOf(c: Char): Int = when {
        c.isLetterOrDigit() || c == '_' -> 0
        c.isWhitespace() -> 1
        else -> 2
    }

    /** Extends the current token from [tokenStart] over one class run. */
    private fun scan() {
        tokenEnd = tokenStart
        if (tokenStart >= end) return
        val cls = classOf(buffer[tokenStart])
        tokenEnd = tokenStart + 1
        while (tokenEnd < end && classOf(buffer[tokenEnd]) == cls) tokenEnd++
    }

    override fun getState() = 0
    override fun getTokenType(): IElementType? {
        if (tokenStart >= end) return null
        return when (classOf(buffer[tokenStart])) {
            0 -> word
            1 -> TokenType.WHITE_SPACE
            else -> other
        }
    }

    override fun getTokenStart() = tokenStart
    override fun getTokenEnd() = tokenEnd
    override fun advance() {
        tokenStart = tokenEnd
        scan()
    }

    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd() = end
}

/** One file node over whatever the lexer produced. */
class WholeTextParser : PsiParser {
    override fun parse(root: IElementType, builder: PsiBuilder): ASTNode {
        val mark = builder.mark()
        while (!builder.eof()) builder.advanceLexer()
        mark.done(root)
        return builder.treeBuilt
    }
}

class PmcParserDefinition : TextOnlyParserDefinition(PmcLanguage, PmcFileType)
class PmaParserDefinition : TextOnlyParserDefinition(PmaLanguage, PmaFileType)
