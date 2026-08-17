package ru.mellonis.tmc

import com.intellij.execution.Executor
import com.intellij.execution.configurations.CommandLineState
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.configurations.LocatableConfigurationBase
import com.intellij.execution.configurations.RunConfigurationOptions
import com.intellij.execution.configurations.RunProfileState
import com.intellij.execution.configurations.RuntimeConfigurationError
import com.intellij.execution.process.OSProcessHandler
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessTerminatedListener
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.JDOMExternalizerUtil
import com.intellij.util.execution.ParametersListUtil
import org.jdom.Element

/**
 * build | compile | asm | lint | run — the `tmt` subcommands offered by
 * the preset combo. `asm` joins the PM-1 plugin's set because TM-1
 * assembly is a first-class authoring surface here, not just a compiler
 * output format.
 */
val TMT_SUBCOMMANDS = listOf("build", "compile", "asm", "lint", "run")

/**
 * A thin `tmt <subcommand> <args>` process wrapper — no build-system
 * ambitions (no compile-before-run graph, no artifact tracking). Runs
 * `TmtSettings.instance.state.tmtPath <subcommand> <parsed args>` in
 * [workingDirectory] and streams output to the run console.
 *
 * The `build` subcommand additionally carries a manifest [target] and the
 * [runAfterBuild] flag, assembling `tmt build [--run] <target> <args>` —
 * the driver resolves the target against the nearest-ancestor `tmt.json`
 * from the working directory. A blank target with sources passed via
 * [arguments] is the driver's argv mode; `--run` is manifest-mode-only
 * and the driver rejects it there with its own message. (A target is
 * `--run`-able only when its manifest run block declares a tape — TM-1
 * has no empty-tape default; that gating too is the driver's.)
 */
class TmtRunConfiguration(project: Project, factory: ConfigurationFactory, name: String) :
    LocatableConfigurationBase<RunConfigurationOptions>(project, factory, name) {

    var subcommand: String = TMT_SUBCOMMANDS.last()
    var arguments: String = ""
    var workingDirectory: String = project.basePath ?: ""
    var target: String = ""
    var runAfterBuild: Boolean = false

    override fun getConfigurationEditor(): SettingsEditor<TmtRunConfiguration> = TmtRunSettingsEditor()

    override fun checkConfiguration() {
        if (subcommand == "build" && target.isBlank() && ParametersListUtil.parse(arguments).isEmpty()) {
            throw RuntimeConfigurationError(
                "Set a target (or pass source files in Arguments) for the build subcommand.")
        }
    }

    override fun getState(executor: Executor, environment: ExecutionEnvironment): RunProfileState =
        object : CommandLineState(environment) {
            override fun startProcess(): ProcessHandler {
                val commandLine = GeneralCommandLine(TmtSettings.instance.state.tmtPath, subcommand)
                if (subcommand == "build") {
                    if (runAfterBuild) {
                        commandLine.addParameter("--run")
                    }
                    if (target.isNotBlank()) {
                        commandLine.addParameter(target)
                    }
                }
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
        JDOMExternalizerUtil.writeField(element, "target", target)
        JDOMExternalizerUtil.writeField(element, "runAfterBuild", runAfterBuild.toString())
    }

    override fun readExternal(element: Element) {
        super.readExternal(element)
        JDOMExternalizerUtil.readField(element, "subcommand")?.let { subcommand = it }
        JDOMExternalizerUtil.readField(element, "arguments")?.let { arguments = it }
        JDOMExternalizerUtil.readField(element, "workingDirectory")?.let { workingDirectory = it }
        JDOMExternalizerUtil.readField(element, "target")?.let { target = it }
        JDOMExternalizerUtil.readField(element, "runAfterBuild")?.let { runAfterBuild = it.toBoolean() }
    }
}
