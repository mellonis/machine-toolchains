package ru.mellonis.tmc

import com.intellij.icons.AllIcons
import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

/**
 * The `.tma` file type. Coloring comes from the TextMate bundle
 * ([TmcTextMateBundleProvider]) through the highlighter registrations in
 * plugin.xml; the backing [TmaLanguage] exists so the platform builds the
 * editor highlighter through the file-type provider lookup (see
 * [TmcLanguages.kt][TmaLanguage] for why a plain FileType cannot color).
 */
object TmaFileType : LanguageFileType(TmaLanguage) {
    override fun getName() = "TMA"
    override fun getDescription() = "TM-1 assembly source"
    override fun getDefaultExtension() = "tma"
    override fun getIcon(): Icon = AllIcons.FileTypes.Text
}
