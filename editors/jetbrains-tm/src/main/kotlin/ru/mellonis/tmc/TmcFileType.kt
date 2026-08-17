package ru.mellonis.tmc

import com.intellij.icons.AllIcons
import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

/**
 * The `.tmc` file type. Coloring comes from the TextMate bundle
 * ([TmcTextMateBundleProvider]) through the highlighter registrations in
 * plugin.xml; the backing [TmcLanguage] exists so the platform builds the
 * editor highlighter through the file-type provider lookup (see
 * [TmcLanguages.kt][TmcLanguage] for why a plain FileType cannot color).
 */
object TmcFileType : LanguageFileType(TmcLanguage) {
    override fun getName() = "TMC"
    override fun getDescription() = "Turing machine toolchain source"
    override fun getDefaultExtension() = "tmc"
    override fun getIcon(): Icon = AllIcons.FileTypes.Text
}
