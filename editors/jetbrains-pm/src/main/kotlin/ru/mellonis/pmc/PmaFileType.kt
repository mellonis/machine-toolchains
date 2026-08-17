package ru.mellonis.pmc

import com.intellij.icons.AllIcons
import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

/**
 * The `.pma` file type. Coloring comes from the TextMate bundle
 * ([PmcTextMateBundleProvider]) through the highlighter registrations in
 * plugin.xml; the backing [PmaLanguage] exists so the platform builds the
 * editor highlighter through the file-type provider lookup (see
 * [PmcLanguages.kt][PmaLanguage] for why a plain FileType cannot color).
 */
object PmaFileType : LanguageFileType(PmaLanguage) {
    override fun getName() = "PMA"
    override fun getDescription() = "PM-1 assembly source"
    override fun getDefaultExtension() = "pma"
    override fun getIcon(): Icon = AllIcons.FileTypes.Text
}
