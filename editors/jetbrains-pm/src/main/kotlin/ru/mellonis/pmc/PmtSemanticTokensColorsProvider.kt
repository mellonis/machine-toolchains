package ru.mellonis.pmc

import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.psi.PsiFile
import com.redhat.devtools.lsp4ij.features.semanticTokens.DefaultSemanticTokensColorsProvider
import com.redhat.devtools.lsp4ij.features.semanticTokens.SemanticTokensColorsProvider

/**
 * Maps `pmt lsp` semantic-token types onto theme-adaptive IntelliJ
 * attributes. The .pmc service emits namespace/function/number and the
 * .pma one function/variable/number, with modifiers `declaration` and
 * `defaultLibrary`; `defaultLibrary` deliberately does not change the
 * key — a stdlib call reads as a call. Anything outside the table
 * delegates to LSP4IJ's default provider so an unmapped future type
 * degrades to default rendering instead of disappearing.
 */
class PmtSemanticTokensColorsProvider : SemanticTokensColorsProvider {
    private val fallback = DefaultSemanticTokensColorsProvider()

    override fun getTextAttributesKey(
        tokenType: String,
        tokenModifiers: List<String>,
        file: PsiFile,
    ): TextAttributesKey? = when (tokenType) {
        "function" ->
            if ("declaration" in tokenModifiers) DefaultLanguageHighlighterColors.FUNCTION_DECLARATION
            else DefaultLanguageHighlighterColors.FUNCTION_CALL
        "namespace" -> DefaultLanguageHighlighterColors.CLASS_NAME
        "type" -> DefaultLanguageHighlighterColors.CLASS_REFERENCE
        "variable" -> DefaultLanguageHighlighterColors.LOCAL_VARIABLE
        "string" -> DefaultLanguageHighlighterColors.STRING
        "number" -> DefaultLanguageHighlighterColors.NUMBER
        else -> fallback.getTextAttributesKey(tokenType, tokenModifiers, file)
    }
}
