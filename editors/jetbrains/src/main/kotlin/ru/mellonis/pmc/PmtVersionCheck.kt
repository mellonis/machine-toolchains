package ru.mellonis.pmc

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

private const val NOTIFICATION_GROUP_ID = "ru.mellonis.pmc"
private const val MIN_TESTED_PMT = "0.1.0"
private const val VERSION_CHECK_TIMEOUT_SECONDS = 5L
private val VERSION_LINE = Regex("""pmt (\d+)\.(\d+)\.(\d+)""")

/**
 * Startup skew check: warns (never blocks) when the configured `pmt`
 * binary is older than [MIN_TESTED_PMT], and reports an error notification
 * pointing at the settings page when the binary can't be found at all.
 * No `.pmc` language knowledge lives here — this only ever reads
 * `pmt --version`'s own text.
 */
class PmtVersionCheck : ProjectActivity {
    override suspend fun execute(project: Project) {
        val pmtPath = PmtSettings.instance.state.pmtPath
        val output = withContext(Dispatchers.IO) { runPmtVersion(pmtPath) }
        if (output == null) {
            notify(
                project,
                NotificationType.ERROR,
                "pmt not found at '$pmtPath' — set the binary path in Settings | Tools | pmt, " +
                    "or install with 'cargo install --path crates/post-machine'.",
            )
            return
        }
        val found = VERSION_LINE.find(output) ?: return
        val (major, minor, patch) = found.destructured
        val foundVersion = listOf(major, minor, patch).map(String::toInt)
        val minVersion = MIN_TESTED_PMT.split(".").map(String::toInt)
        if (older(foundVersion, minVersion)) {
            notify(
                project,
                NotificationType.WARNING,
                "pmt $major.$minor.$patch is older than the tested $MIN_TESTED_PMT; " +
                    "some features may misbehave — update pmt.",
            )
        }
    }

    private fun runPmtVersion(pmtPath: String): String? {
        return try {
            val process = ProcessBuilder(pmtPath, "--version").redirectErrorStream(true).start()
            // Drain stdout on a background thread instead of blocking here:
            // reading synchronously would block until the child closes
            // stdout, so a misconfigured `pmtPath` pointing at a
            // non-terminating process (or a shell wrapper that never
            // exits) would hold the pipe open and the waitFor timeout below
            // would never be reached. `waitFor` is what actually bounds
            // this call; on timeout the process is force-killed and this
            // future is simply abandoned — `destroyForcibly` closes the
            // child's stdout, so the abandoned reader hits EOF and its
            // background thread exits on its own; nothing joins it either
            // way.
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
