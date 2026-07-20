package ru.mellonis.tmc

import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.ui.ComboBox
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import javax.swing.JComponent

/** The tmt run-configuration editor: subcommand preset combo + args + working directory. */
class TmtRunSettingsEditor : SettingsEditor<TmtRunConfiguration>() {
    private val subcommandCombo = ComboBox(TMT_SUBCOMMANDS.toTypedArray())
    private val argumentsField = JBTextField()
    private val workingDirectoryField = JBTextField()

    override fun resetEditorFrom(config: TmtRunConfiguration) {
        subcommandCombo.selectedItem = config.subcommand
        argumentsField.text = config.arguments
        workingDirectoryField.text = config.workingDirectory
    }

    override fun applyEditorTo(config: TmtRunConfiguration) {
        config.subcommand = subcommandCombo.selectedItem as? String ?: TMT_SUBCOMMANDS.last()
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
