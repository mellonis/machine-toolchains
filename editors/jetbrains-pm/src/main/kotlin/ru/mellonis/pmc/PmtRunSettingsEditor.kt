package ru.mellonis.pmc

import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.ui.ComboBox
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import javax.swing.JComponent

/** The pmt run-configuration editor: subcommand preset combo + args + working directory. */
class PmtRunSettingsEditor : SettingsEditor<PmtRunConfiguration>() {
    private val subcommandCombo = ComboBox(PMT_SUBCOMMANDS.toTypedArray())
    private val argumentsField = JBTextField()
    private val workingDirectoryField = JBTextField()

    override fun resetEditorFrom(config: PmtRunConfiguration) {
        subcommandCombo.selectedItem = config.subcommand
        argumentsField.text = config.arguments
        workingDirectoryField.text = config.workingDirectory
    }

    override fun applyEditorTo(config: PmtRunConfiguration) {
        config.subcommand = subcommandCombo.selectedItem as? String ?: PMT_SUBCOMMANDS.last()
        config.arguments = argumentsField.text
        config.workingDirectory = workingDirectoryField.text
    }

    override fun createEditor(): JComponent =
        FormBuilder.createFormBuilder()
            .addLabeledComponent("Subcommand:", subcommandCombo)
            .addLabeledComponent("Arguments:", argumentsField)
            .addLabeledComponent("Working directory:", workingDirectoryField)
            .panel
}
