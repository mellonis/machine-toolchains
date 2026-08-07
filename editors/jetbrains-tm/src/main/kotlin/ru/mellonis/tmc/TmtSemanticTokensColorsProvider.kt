package ru.mellonis.tmc

import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.psi.PsiFile
import com.redhat.devtools.lsp4ij.features.semanticTokens.DefaultSemanticTokensColorsProvider
import com.redhat.devtools.lsp4ij.features.semanticTokens.SemanticTokensColorsProvider

/**
 * Maps `tmt lsp` semantic-token types onto theme-adaptive IntelliJ
 * attributes. The .tmc service emits namespace/type/function/variable/
 * string/number and the .tma one a subset, both with the `declaration`
 * modifier; anything outside the table delegates to LSP4IJ's default
 * provider so an unmapped future type degrades to default rendering
 * instead of disappearing.
 */
class TmtSemanticTokensColorsProvider : SemanticTokensColorsProvider {
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
