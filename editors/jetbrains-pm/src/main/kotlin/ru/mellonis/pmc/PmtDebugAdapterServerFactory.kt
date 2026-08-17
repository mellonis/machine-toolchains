package ru.mellonis.pmc

import com.intellij.execution.ExecutionException
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.execution.configurations.RunConfigurationOptions
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.redhat.devtools.lsp4ij.dap.DebugMode
import com.redhat.devtools.lsp4ij.dap.LaunchConfiguration
import com.redhat.devtools.lsp4ij.dap.configurations.DAPRunConfiguration
import com.redhat.devtools.lsp4ij.dap.configurations.DAPRunConfigurationOptions
import com.redhat.devtools.lsp4ij.dap.definitions.DebugAdapterServerDefinition
import com.redhat.devtools.lsp4ij.dap.descriptors.DebugAdapterDescriptor
import com.redhat.devtools.lsp4ij.dap.descriptors.DebugAdapterDescriptorFactory
import com.redhat.devtools.lsp4ij.dap.descriptors.DefaultDebugAdapterDescriptor

/** The file extensions gutter breakpoints and "debug this file" accept. */
private val DEBUGGABLE_EXTENSIONS = setOf("pmc", "pma")

/**
 * The two launch shapes `pmt dap` recognizes, as editable JSON templates
 * for the DAP run configuration's launch tab. The `type`/`request`/`name`
 * keys are inert for the adapter (it reads only its own arguments) but
 * keep the JSON copy-pasteable to and from a VS Code `launch.json`.
 * `${workspaceFolder}` resolves to the configuration's working directory.
 */
private val LAUNCH_TARGET = LaunchConfiguration(
    "pmt-launch-target",
    "pmt: launch target",
    """
    {
      "type": "pmt",
      "request": "launch",
      "name": "pmt: launch target",
      "target": "main",
      "stopOnEntry": true
    }
    """.trimIndent(),
    DebugMode.LAUNCH,
)
private val LAUNCH_PROGRAM = LaunchConfiguration(
    "pmt-launch-program",
    "pmt: launch program",
    """
    {
      "type": "pmt",
      "request": "launch",
      "name": "pmt: launch program",
      "program": "${'$'}{workspaceFolder}/main.pmx",
      "tape": "${'$'}{workspaceFolder}/main.pmt",
      "stopOnEntry": true
    }
    """.trimIndent(),
    DebugMode.LAUNCH,
)

/**
 * Registers `pmt dap` as an LSP4IJ debug adapter server, bridging it into
 * the IDE's XDebugger framework (gutter breakpoints in `.pmc`/`.pma`
 * files, stepping, threads/variables views). The adapter itself is
 * editor-agnostic — the same stdio server the VS Code extension consumes;
 * launch semantics (target mode vs program mode) live in the binary and
 * are documented in this repository's `docs/dap.md`.
 */
class PmtDebugAdapterServerFactory : DebugAdapterDescriptorFactory() {

    override fun createDebugAdapterDescriptor(
        options: RunConfigurationOptions,
        environment: ExecutionEnvironment,
    ): DebugAdapterDescriptor = PmtDebugAdapterDescriptor(options, environment, serverDefinition)

    override fun isDebuggableFile(file: VirtualFile, project: Project): Boolean =
        file.extension?.lowercase() in DEBUGGABLE_EXTENSIONS

    override fun getLaunchConfigurations(): List<LaunchConfiguration> =
        listOf(LAUNCH_TARGET, LAUNCH_PROGRAM)

    /**
     * Seeds a DAP run configuration created from a `.pmc`/`.pma` context
     * (the platform's "create from file" flow). On top of what the base
     * fills (file, name, launch mode, server identity): the `pmt dap`
     * command from the settings-page binary path, the project root as the
     * working directory — the adapter's own manifest discovery walks up
     * from its cwd, so this is what makes target mode find `pmt.json` —
     * and target mode as the selected launch template.
     */
    override fun prepareConfiguration(
        configuration: RunConfiguration,
        file: VirtualFile,
        project: Project,
    ): Boolean {
        if (!super.prepareConfiguration(configuration, file, project)) return false
        (configuration as? DAPRunConfiguration)?.let { config ->
            config.command = defaultDapCommand()
            config.workingDirectory = project.basePath.orEmpty()
            config.launchConfigurationId = LAUNCH_TARGET.id
            config.launchConfiguration = LAUNCH_TARGET.content
        }
        return true
    }
}

/**
 * The default descriptor with two gaps closed for this adapter:
 *
 * - A blank command field falls back to `<settings binary path> dap`
 *   instead of failing the launch, so a hand-created configuration works
 *   without retyping the path the settings page already holds.
 * - The server process always gets a working directory (the
 *   configuration's, else the project root). The default leaves it unset
 *   — inheriting the IDE process's own cwd — which would break target
 *   mode's nearest-ancestor `pmt.json` discovery and relative
 *   `program`/`tape` paths alike.
 *
 * Transport stays the default stdio: the command carries no port
 * placeholder,
 * so LSP4IJ speaks DAP over the spawned process's stdin/stdout — exactly
 * how `pmt dap` is deployed under VS Code.
 */
class PmtDebugAdapterDescriptor(
    options: RunConfigurationOptions,
    environment: ExecutionEnvironment,
    serverDefinition: DebugAdapterServerDefinition,
) : DefaultDebugAdapterDescriptor(options, environment, serverDefinition) {

    override fun startServer(): ProcessHandler {
        val opts = options as? DAPRunConfigurationOptions
        val env = envData
        val fromOptions =
            if (opts?.command.isNullOrBlank()) null else createStartServerCommandLine(options)
        val commandLine: GeneralCommandLine = fromOptions
            ?: createStartServerCommandLine(
                defaultDapCommand(),
                env?.envs ?: emptyMap(),
                env?.isPassParentEnvs ?: true,
            )
            ?: throw ExecutionException("the pmt dap command line could not be built")
        if (commandLine.workDirectory == null) {
            val cwd = opts?.workingDirectory?.takeIf { it.isNotBlank() }
                ?: environment.project.basePath
            if (cwd != null) {
                commandLine.withWorkDirectory(cwd)
            }
        }
        return startServer(commandLine)
    }
}

/**
 * `<binary path> dap`, quoting the path when it contains whitespace —
 * the command string is shell-style-split before spawning.
 */
internal fun defaultDapCommand(): String {
    val path = PmtSettings.instance.state.pmtPath
    val quoted = if (path.any(Char::isWhitespace)) "\"$path\"" else path
    return "$quoted dap"
}
