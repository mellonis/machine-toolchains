package ru.mellonis.pmc

import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.ConfigurationTypeBase
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.icons.AllIcons
import com.intellij.openapi.project.Project

private const val PMT_RUN_TYPE_ID = "PmtRun"

/**
 * The `pmt` run-configuration type: id "PmtRun", display "pmt", one
 * factory. A thin process wrapper around the `pmt` binary — no
 * build-system ambitions (see [PmtRunConfiguration]).
 */
class PmtRunConfigurationType : ConfigurationTypeBase(
    PMT_RUN_TYPE_ID,
    "pmt",
    "Run a pmt subcommand (compile, lint, run) against a file or object.",
    AllIcons.RunConfigurations.Application,
) {
    init {
        addFactory(PmtRunConfigurationFactory(this))
    }
}

class PmtRunConfigurationFactory(type: PmtRunConfigurationType) : ConfigurationFactory(type) {
    override fun getId(): String = PMT_RUN_TYPE_ID

    override fun createTemplateConfiguration(project: Project): RunConfiguration =
        PmtRunConfiguration(project, this, "pmt")
}
