package ru.mellonis.tmc

import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.IOException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

private const val NOTIFICATION_GROUP_ID = "ru.mellonis.tmc"

/**
 * The oldest `tmt` this plugin targets as its tested floor. A binary
 * reporting older gets a warning, never a hard failure — the plugin is a
 * thin client and an older server simply answers less. Bump this in the
 * same commit that raises the plugin's own version whenever a newly
 * required server capability lands. The VS Code extension carries the
 * same constant under the same name; the two must move together.
 */
private const val MIN_TESTED_TMT = "0.2.0"
private const val VERSION_CHECK_TIMEOUT_SECONDS = 5L
private val VERSION_LINE = Regex("""tmt (\d+)\.(\d+)\.(\d+)""")

/**
 * Startup skew check: warns (never blocks) when the configured `tmt`
 * binary is older than [MIN_TESTED_TMT], and reports an error notification
 * pointing at the settings page when the binary can't be found at all.
 * No `.tmc` language knowledge lives here — this only ever reads
 * `tmt --version`'s own text.
 */
class TmtVersionCheck : ProjectActivity {
    override suspend fun execute(project: Project) {
        val tmtPath = TmtSettings.instance.state.tmtPath
        val output = withContext(Dispatchers.IO) { runTmtVersion(tmtPath) }
        if (output == null) {
            notify(
                project,
                NotificationType.ERROR,
                "tmt not found at '$tmtPath' — set the binary path in Settings | Tools | tmt, " +
                    "or install with 'cargo install --path crates/turing-machine'.",
            )
            return
        }
        val found = VERSION_LINE.find(output) ?: return
        val (major, minor, patch) = found.destructured
        val foundVersion = listOf(major, minor, patch).map(String::toInt)
        val minVersion = MIN_TESTED_TMT.split(".").map(String::toInt)
        if (older(foundVersion, minVersion)) {
            notify(
                project,
                NotificationType.WARNING,
                "tmt $major.$minor.$patch is older than the tested $MIN_TESTED_TMT; " +
                    "some features may misbehave — update tmt.",
            )
        }
    }

    private fun runTmtVersion(tmtPath: String): String? {
        return try {
            val process = ProcessBuilder(tmtPath, "--version").redirectErrorStream(true).start()
            // Drain stdout on a background thread instead of blocking here:
            // reading synchronously would block until the child closes
            // stdout, so a misconfigured `tmtPath` pointing at a
            // non-terminating process (or a shell wrapper that never exits)
            // would hold the pipe open and the waitFor timeout below would
            // never be reached. `waitFor` is what actually bounds this call;
            // on timeout the process is force-killed and this future is
            // simply abandoned — `destroyForcibly` closes the child's stdout,
            // so the abandoned reader hits EOF and its background thread
            // exits on its own; nothing joins it either way.
            val outputFuture = CompletableFuture.supplyAsync {
                process.inputStream.bufferedReader().readText()
            }
            if (!process.waitFor(VERSION_CHECK_TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
                process.destroyForcibly()
                return null
            }
            // The process has already exited, so stdout is closed and the
            // drain above finishes essentially immediately; this timeout is
            // defensive, not load-bearing.
            outputFuture.get(VERSION_CHECK_TIMEOUT_SECONDS, TimeUnit.SECONDS)
        } catch (e: IOException) {
            null
        } catch (e: TimeoutException) {
            null
        } catch (e: ExecutionException) {
            null
        } catch (e: InterruptedException) {
            Thread.currentThread().interrupt()
            null
        }
    }

    private fun older(found: List<Int>, min: List<Int>): Boolean {
        for (i in 0..2) {
            if (found[i] != min[i]) return found[i] < min[i]
        }
        return false
    }

    private fun notify(project: Project, type: NotificationType, content: String) {
        NotificationGroupManager.getInstance()
            .getNotificationGroup(NOTIFICATION_GROUP_ID)
            .createNotification(content, type)
            .notify(project)
    }
}
