package ru.mellonis.pmc

import com.intellij.icons.AllIcons
import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

/**
 * The `.pmc` file type. Coloring comes from the TextMate bundle
 * ([PmcTextMateBundleProvider]) through the highlighter registrations in
 * plugin.xml; the backing [PmcLanguage] exists so the platform builds the
 * editor highlighter through the file-type provider lookup (see
 * [PmcLanguages.kt][PmcLanguage] for why a plain FileType cannot color).
 */
object PmcFileType : LanguageFileType(PmcLanguage) {
    override fun getName() = "PMC"
    override fun getDescription() = "Post machine toolchain source"
    override fun getDefaultExtension() = "pmc"
    override fun getIcon(): Icon = AllIcons.FileTypes.Text
}
