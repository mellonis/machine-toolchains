package ru.mellonis.tmc

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.process.CapturingProcessHandler

private const val LIST_TARGETS_TIMEOUT_MS = 10_000

/** One entry of `tmt build --list-targets` output. */
data class TmtTarget(val name: String, val runnable: Boolean)

/**
 * Lists the project manifest's declared targets by running
 * `tmt build --list-targets` in [workingDirectory] — the driver does its
 * own nearest-ancestor `tmt.json` discovery from there, so the plugin
 * never looks for the manifest itself. Blocking (call from a background
 * thread); throws with the binary's own message on any failure — no
 * manifest, an invalid one, or a missing binary.
 */
fun listTmtTargets(workingDirectory: String): List<TmtTarget> {
    val commandLine =
        GeneralCommandLine(TmtSettings.instance.state.tmtPath, "build", "--list-targets")
    if (workingDirectory.isNotBlank()) {
        commandLine.withWorkDirectory(workingDirectory)
    }
    val output = CapturingProcessHandler(commandLine).runProcess(LIST_TARGETS_TIMEOUT_MS)
    if (output.isTimeout) {
        throw RuntimeException("tmt build --list-targets timed out")
    }
    if (output.exitCode != 0) {
        throw RuntimeException(output.stderr.trim().ifEmpty { "exit ${output.exitCode}" })
    }
    return parseTargets(output.stdout)
}

/**
 * Parses `--list-targets` stdout: one line per target, the name
 * optionally followed by a TAB and the literal `run` when the target
 * declares a run block. The format is pinned by the crate's build-driver
 * tests.
 */
internal fun parseTargets(stdout: String): List<TmtTarget> =
    stdout.lineSequence()
        .filter { it.isNotEmpty() }
        .map { line ->
            val pieces = line.split('\t')
            TmtTarget(pieces[0], pieces.getOrNull(1) == "run")
        }
        .filter { it.name.isNotEmpty() }
        .toList()
