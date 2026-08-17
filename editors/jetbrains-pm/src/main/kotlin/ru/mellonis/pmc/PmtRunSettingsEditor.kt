package ru.mellonis.pmc

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.ui.ComboBox
import com.intellij.ui.components.JBCheckBox
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBTextField
import com.intellij.util.concurrency.AppExecutorUtil
import com.intellij.util.ui.FormBuilder
import java.awt.BorderLayout
import javax.swing.DefaultComboBoxModel
import javax.swing.JButton
import javax.swing.JComponent
import javax.swing.JPanel

/**
 * The pmt run-configuration editor: subcommand preset combo + args +
 * working directory, plus the build-only row — an editable target combo
 * (fed by `pmt build --list-targets`, run in the working directory on
 * demand via the refresh button, never automatically) and the `--run`
 * checkbox. The build row is disabled for the other subcommands.
 */
class PmtRunSettingsEditor : SettingsEditor<PmtRunConfiguration>() {
    private val subcommandCombo = ComboBox(PMT_SUBCOMMANDS.toTypedArray())
    private val argumentsField = JBTextField()
    private val workingDirectoryField = JBTextField()
    private val targetCombo = ComboBox<String>().apply { isEditable = true }
    private val refreshTargetsButton = JButton("Refresh").apply {
        toolTipText = "Re-read the target list (pmt build --list-targets in the working directory)"
    }
    private val targetStatusLabel = JBLabel("")
    private val runAfterBuildCheckBox = JBCheckBox("Run the target after building (--run)")

    init {
        subcommandCombo.addActionListener { updateBuildRowState() }
        refreshTargetsButton.addActionListener { refreshTargets() }
    }

    override fun resetEditorFrom(config: PmtRunConfiguration) {
        subcommandCombo.selectedItem = config.subcommand
        argumentsField.text = config.arguments
        workingDirectoryField.text = config.workingDirectory
        targetCombo.selectedItem = config.target
        runAfterBuildCheckBox.isSelected = config.runAfterBuild
        updateBuildRowState()
    }

    override fun applyEditorTo(config: PmtRunConfiguration) {
        config.subcommand = subcommandCombo.selectedItem as? String ?: PMT_SUBCOMMANDS.last()
        config.arguments = argumentsField.text
        config.workingDirectory = workingDirectoryField.text
        config.target = editedTarget()
        config.runAfterBuild = runAfterBuildCheckBox.isSelected
    }

    override fun createEditor(): JComponent {
        val targetRow = JPanel(BorderLayout(4, 0)).apply {
            add(targetCombo, BorderLayout.CENTER)
            add(refreshTargetsButton, BorderLayout.EAST)
        }
        return FormBuilder.createFormBuilder()
            .addLabeledComponent("Subcommand:", subcommandCombo)
            .addLabeledComponent("Target:", targetRow)
            .addComponentToRightColumn(targetStatusLabel)
            .addComponentToRightColumn(runAfterBuildCheckBox)
            .addLabeledComponent("Arguments:", argumentsField)
            .addLabeledComponent("Working directory:", workingDirectoryField)
            .panel
    }

    /** The combo's current text — the edited value, not just a picked item. */
    private fun editedTarget(): String =
        (if (targetCombo.isEditable) targetCombo.editor.item else targetCombo.selectedItem)
            ?.toString()?.trim().orEmpty()

    private fun updateBuildRowState() {
        val isBuild = subcommandCombo.selectedItem == "build"
        targetCombo.isEnabled = isBuild
        refreshTargetsButton.isEnabled = isBuild
        runAfterBuildCheckBox.isEnabled = isBuild
    }

    /**
     * Off-EDT process run, EDT model update. The refresh preserves the
     * edited target text — a typed name need not be in the fresh list
     * (the manifest may have changed since, or the binary may be
     * missing), and failures cost only this refresh: the error lands in
     * the status label and the previous items stay.
     */
    private fun refreshTargets() {
        val cwd = workingDirectoryField.text
        refreshTargetsButton.isEnabled = false
        targetStatusLabel.text = "Listing targets…"
        AppExecutorUtil.getAppExecutorService().submit {
            val result = runCatching { listPmtTargets(cwd) }
            ApplicationManager.getApplication().invokeLater {
                refreshTargetsButton.isEnabled = subcommandCombo.selectedItem == "build"
                result.fold(
                    onSuccess = { targets ->
                        val edited = editedTarget()
                        targetCombo.model = DefaultComboBoxModel(targets.map { it.name }.toTypedArray())
                        targetCombo.selectedItem = edited.ifEmpty { targets.firstOrNull()?.name.orEmpty() }
                        val runnable = targets.count { it.runnable }
                        targetStatusLabel.text =
                            "${targets.size} target(s), $runnable with a run block"
                    },
                    onFailure = { err ->
                        targetStatusLabel.text = (err.message ?: "failed").lineSequence().first()
                    },
                )
            }
        }
    }
}
