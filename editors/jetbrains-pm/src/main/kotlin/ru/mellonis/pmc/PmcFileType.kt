package ru.mellonis.pmc

import com.intellij.icons.AllIcons
import com.intellij.openapi.fileTypes.FileType
import javax.swing.Icon

/**
 * The `.pmc` file type. Coloring comes from the TextMate bundle
 * ([PmcTextMateBundleProvider]), not from this type — see the
 * `editorHighlighterProvider` registration in plugin.xml for why both are
 * needed together.
 */
object PmcFileType : FileType {
    override fun getName() = "PMC"
    override fun getDescription() = "Post machine toolchain source"
    override fun getDefaultExtension() = "pmc"
    override fun getIcon(): Icon = AllIcons.FileTypes.Text
    override fun isBinary() = false
}
