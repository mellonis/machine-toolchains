package ru.mellonis.pmc

import com.intellij.icons.AllIcons
import com.intellij.openapi.fileTypes.FileType
import javax.swing.Icon

/**
 * The `.pma` file type. Coloring comes from the TextMate bundle
 * ([PmcTextMateBundleProvider]), not from this type — see the
 * `editorHighlighterProvider` registration in plugin.xml for why both are
 * needed together.
 */
object PmaFileType : FileType {
    override fun getName() = "PMA"
    override fun getDescription() = "PM-1 assembly source"
    override fun getDefaultExtension() = "pma"
    override fun getIcon(): Icon = AllIcons.FileTypes.Text
    override fun isBinary() = false
}
