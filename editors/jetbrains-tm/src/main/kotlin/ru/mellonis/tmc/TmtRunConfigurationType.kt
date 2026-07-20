package ru.mellonis.tmc

import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.ConfigurationTypeBase
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.icons.AllIcons
import com.intellij.openapi.project.Project

private const val TMT_RUN_TYPE_ID = "TmtRun"

/**
 * The `tmt` run-configuration type: id "TmtRun", display "tmt", one
 * factory. A thin process wrapper around the `tmt` binary — no
 * build-system ambitions (see [TmtRunConfiguration]).
 */
class TmtRunConfigurationType : ConfigurationTypeBase(
    TMT_RUN_TYPE_ID,
    "tmt",
    "Run a tmt subcommand (compile, asm, lint, run) against a file or object.",
    AllIcons.RunConfigurations.Application,
) {
    init {
        addFactory(TmtRunConfigurationFactory(this))
    }
}

class TmtRunConfigurationFactory(type: TmtRunConfigurationType) : ConfigurationFactory(type) {
    override fun getId(): String = TMT_RUN_TYPE_ID

    override fun createTemplateConfiguration(project: Project): RunConfiguration =
        TmtRunConfiguration(project, this, "tmt")
}
