package ru.mellonis.pmc

import com.intellij.execution.Executor
import com.intellij.execution.configurations.CommandLineState
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.configurations.LocatableConfigurationBase
import com.intellij.execution.configurations.RunConfigurationOptions
import com.intellij.execution.configurations.RunProfileState
import com.intellij.execution.process.OSProcessHandler
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessTerminatedListener
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.JDOMExternalizerUtil
import com.intellij.util.execution.ParametersListUtil
import org.jdom.Element

/** compile | lint | run — the `pmt` subcommands offered by the preset combo. */
val PMT_SUBCOMMANDS = listOf("compile", "lint", "run")

/**
 * A thin `pmt <subcommand> <args>` process wrapper — no build-system
 * ambitions (no compile-before-run graph, no artifact tracking). Runs
 * `PmtSettings.instance.state.pmtPath <subcommand> <parsed args>` in
 * [workingDirectory] and streams output to the run console.
 */
class PmtRunConfiguration(project: Project, factory: ConfigurationFactory, name: String) :
    LocatableConfigurationBase<RunConfigurationOptions>(project, factory, name) {

    var subcommand: String = PMT_SUBCOMMANDS.last()
    var arguments: String = ""
    var workingDirectory: String = project.basePath ?: ""

    override fun getConfigurationEditor(): SettingsEditor<PmtRunConfiguration> = PmtRunSettingsEditor()

    override fun getState(executor: Executor, environment: ExecutionEnvironment): RunProfileState =
        object : CommandLineState(environment) {
            override fun startProcess(): ProcessHandler {
                val commandLine = GeneralCommandLine(PmtSettings.instance.state.pmtPath, subcommand)
                val parsedArgs = ParametersListUtil.parse(arguments)
                if (parsedArgs.isNotEmpty()) {
                    commandLine.addParameters(parsedArgs)
                }
                if (workingDirectory.isNotBlank()) {
                    commandLine.withWorkDirectory(workingDirectory)
                }
                val handler = OSProcessHandler(commandLine)
                ProcessTerminatedListener.attach(handler)
                return handler
            }
        }

    override fun writeExternal(element: Element) {
        super.writeExternal(element)
        JDOMExternalizerUtil.writeField(element, "subcommand", subcommand)
        JDOMExternalizerUtil.writeField(element, "arguments", arguments)
        JDOMExternalizerUtil.writeField(element, "workingDirectory", workingDirectory)
    }

    override fun readExternal(element: Element) {
        super.readExternal(element)
        JDOMExternalizerUtil.readField(element, "subcommand")?.let { subcommand = it }
        JDOMExternalizerUtil.readField(element, "arguments")?.let { arguments = it }
        JDOMExternalizerUtil.readField(element, "workingDirectory")?.let { workingDirectory = it }
    }
}
