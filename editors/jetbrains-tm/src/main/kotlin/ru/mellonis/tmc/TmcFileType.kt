package ru.mellonis.tmc

import com.intellij.icons.AllIcons
import com.intellij.openapi.fileTypes.FileType
import javax.swing.Icon

/**
 * The `.tmc` file type. Coloring comes from the TextMate bundle
 * ([TmcTextMateBundleProvider]), not from this type — see the
 * `editorHighlighterProvider` registration in plugin.xml for why both are
 * needed together.
 */
object TmcFileType : FileType {
    override fun getName() = "TMC"
    override fun getDescription() = "Turing machine toolchain source"
    override fun getDefaultExtension() = "tmc"
    override fun getIcon(): Icon = AllIcons.FileTypes.Text
    override fun isBinary() = false
}
