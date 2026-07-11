package ru.mellonis.pmc

import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.IOException

private const val NOTIFICATION_GROUP_ID = "ru.mellonis.pmc"
private const val MIN_TESTED_PMT = "0.1.0"
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

    private fun runPmtVersion(pmtPath: String): String? = try {
        val process = ProcessBuilder(pmtPath, "--version").redirectErrorStream(true).start()
        val text = process.inputStream.bufferedReader().readText()
        process.waitFor()
        text
    } catch (e: IOException) {
        null
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
