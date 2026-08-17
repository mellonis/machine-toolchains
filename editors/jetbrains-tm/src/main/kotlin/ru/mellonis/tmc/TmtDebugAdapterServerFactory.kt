package ru.mellonis.tmc

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
private val DEBUGGABLE_EXTENSIONS = setOf("tmc", "tma")

/**
 * The two launch shapes `tmt dap` recognizes, as editable JSON templates
 * for the DAP run configuration's launch tab. Program mode REQUIRES the
 * `tape` key — TM-1 has no empty-tape default (unlike PM-1's `pmt`).
 * The `type`/`request`/`name` keys are inert for the adapter (it reads
 * only its own arguments) but keep the JSON copy-pasteable to and from a
 * VS Code `launch.json`. `${workspaceFolder}` resolves to the
 * configuration's working directory.
 */
private val LAUNCH_TARGET = LaunchConfiguration(
    "tmt-launch-target",
    "tmt: launch target",
    """
    {
      "type": "tmt",
      "request": "launch",
      "name": "tmt: launch target",
      "target": "main",
      "stopOnEntry": true
    }
    """.trimIndent(),
    DebugMode.LAUNCH,
)
private val LAUNCH_PROGRAM = LaunchConfiguration(
    "tmt-launch-program",
    "tmt: launch program",
    """
    {
      "type": "tmt",
      "request": "launch",
      "name": "tmt: launch program",
      "program": "${'$'}{workspaceFolder}/main.tmx",
      "tape": "${'$'}{workspaceFolder}/main.tmt",
      "stopOnEntry": true
    }
    """.trimIndent(),
    DebugMode.LAUNCH,
)

/**
 * Registers `tmt dap` as an LSP4IJ debug adapter server, bridging it into
 * the IDE's XDebugger framework (gutter breakpoints in `.tmc`/`.tma`
 * files, stepping, threads/variables views). The adapter itself is
 * editor-agnostic — the same stdio server the VS Code extension consumes;
 * launch semantics (target mode vs program mode) live in the binary and
 * are documented in this repository's `docs/dap.md`.
 */
class TmtDebugAdapterServerFactory : DebugAdapterDescriptorFactory() {

    override fun createDebugAdapterDescriptor(
        options: RunConfigurationOptions,
        environment: ExecutionEnvironment,
    ): DebugAdapterDescriptor = TmtDebugAdapterDescriptor(options, environment, serverDefinition)

    override fun isDebuggableFile(file: VirtualFile, project: Project): Boolean =
        file.extension?.lowercase() in DEBUGGABLE_EXTENSIONS

    override fun getLaunchConfigurations(): List<LaunchConfiguration> =
        listOf(LAUNCH_TARGET, LAUNCH_PROGRAM)

    /**
     * Seeds a DAP run configuration created from a `.tmc`/`.tma` context
     * (the platform's "create from file" flow). On top of what the base
     * fills (file, name, launch mode, server identity): the `tmt dap`
     * command from the settings-page binary path, the project root as the
     * working directory — the adapter's own manifest discovery walks up
     * from its cwd, so this is what makes target mode find `tmt.json` —
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
 *   mode's nearest-ancestor `tmt.json` discovery and relative
 *   `program`/`tape` paths alike.
 *
 * Transport stays the default stdio: the command carries no port
 * placeholder, so LSP4IJ speaks DAP over the spawned process's
 * stdin/stdout — exactly how `tmt dap` is deployed under VS Code.
 */
class TmtDebugAdapterDescriptor(
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
            ?: throw ExecutionException("the tmt dap command line could not be built")
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
    val path = TmtSettings.instance.state.tmtPath
    val quoted = if (path.any(Char::isWhitespace)) "\"$path\"" else path
    return "$quoted dap"
}
