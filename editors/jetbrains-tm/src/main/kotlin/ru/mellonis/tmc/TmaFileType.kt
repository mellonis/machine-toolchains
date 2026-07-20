package ru.mellonis.tmc

import com.intellij.icons.AllIcons
import com.intellij.openapi.fileTypes.FileType
import javax.swing.Icon

/**
 * The `.tma` file type. Coloring comes from the TextMate bundle
 * ([TmcTextMateBundleProvider]), not from this type — see the
 * `editorHighlighterProvider` registration in plugin.xml for why both are
 * needed together.
 */
object TmaFileType : FileType {
    override fun getName() = "TMA"
    override fun getDescription() = "TM-1 assembly source"
    override fun getDefaultExtension() = "tma"
    override fun getIcon(): Icon = AllIcons.FileTypes.Text
    override fun isBinary() = false
}
