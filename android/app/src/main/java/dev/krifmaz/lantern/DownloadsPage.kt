package dev.krifmaz.lantern

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.krifmaz.kit.compose.core.formatAgo
import dev.krifmaz.kit.compose.dialogs.ConfirmDialog
import dev.krifmaz.kit.compose.settings.Muted
import dev.krifmaz.kit.compose.settings.serverCall
import dev.krifmaz.kit.compose.theme.palette
import dev.krifmaz.kit.compose.widgets.BackBar
import dev.krifmaz.kit.compose.widgets.Badge
import dev.krifmaz.kit.compose.widgets.ChoiceRow
import dev.krifmaz.kit.compose.widgets.EmptyState
import dev.krifmaz.kit.compose.widgets.ErrorBox
import dev.krifmaz.kit.compose.widgets.Gutter
import dev.krifmaz.kit.compose.widgets.RowDivider
import dev.krifmaz.kit.compose.widgets.SectionHead
import dev.krifmaz.kit.compose.widgets.SmallButton
import dev.krifmaz.kit.compose.widgets.Spinner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import dev.krifmaz.kit.compose.R as KitR

/** How often the Downloads page asks again while something is still downloading, in milliseconds. */
private const val BUSY_REFRESH_MS = 5_000L

/** How often it asks again when nothing is moving, in milliseconds. */
private const val IDLE_REFRESH_MS = 30_000L

/**
 * The Downloads page: what Lantern downloads for the apps, newest first, with progress, speed and time left, and
 * what went wrong. It asks again every few seconds while something is moving. Admins can try a failed download again
 * and remove one (its files stay on the disk).
 *
 * @param app the app (its server).
 * @param admin whether the account may change downloads (null while not known: the buttons show, and the server
 *   says no if it must).
 * @param appId only this app's downloads (null: every app's).
 * @param appName that app's name, for the title.
 * @param onBack back to the page below.
 */
@Composable
fun DownloadsPage(app: App, admin: Boolean?, appId: Long?, appName: String?, onBack: () -> Unit) {
    val scope = rememberCoroutineScope()
    var list by remember { mutableStateOf<List<Grab>?>(null) }
    var error by remember { mutableStateOf("") }
    var round by remember { mutableIntStateOf(0) }
    var filter by rememberSaveable { mutableIntStateOf(0) }
    var askRemove by remember { mutableStateOf<Grab?>(null) }
    LaunchedEffect(round) {
        while (true) {
            try {
                list = withContext(Dispatchers.IO) { DownloadRules.grabsOf(app.server.getObject("/api/hub/grabs?limit=200")) }
                error = ""
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message ?: "The downloads couldn't be loaded"
            }
            delay(if (list.orEmpty().any { DownloadRules.busy(it) }) BUSY_REFRESH_MS else IDLE_REFRESH_MS)
        }
    }
    Column(Modifier.fillMaxSize()) {
        BackBar(if (appName.isNullOrEmpty()) "Downloads" else "Downloads for $appName", onBack)
        val all = list
        when {
            all == null && error.isNotEmpty() -> ErrorBox("The downloads couldn't be loaded: $error") { round += 1 }

            all == null -> Row(Modifier.fillMaxWidth().padding(Gutter), horizontalArrangement = Arrangement.Center) { Spinner(24.dp) }

            else -> {
                ChoiceRow(GrabFilter.entries.map { it.label }, filter, { filter = it }, Modifier.padding(start = Gutter, top = 8.dp, bottom = 8.dp))
                if (error.isNotEmpty()) Muted("The last refresh didn't work: $error", Modifier.padding(horizontal = Gutter))
                val shown = DownloadRules.filter(all, GrabFilter.entries[filter], appId)
                if (shown.isEmpty()) {
                    EmptyState(KitR.drawable.kit_ic_download, "Nothing here", "Downloads show up here when an app grabs a release through Lantern.")
                } else {
                    val now = System.currentTimeMillis() / 1000.0
                    LazyColumn(Modifier.fillMaxSize()) {
                        items(shown, key = { it.id }) { g ->
                            GrabItem(
                                g,
                                now,
                                canChange = admin != false,
                                onRetry = {
                                    scope.serverCall<Any?>(ok = "Sent again", after = { round += 1 }) {
                                        app.server.post("/api/hub/grabs/${g.id}/retry", JsonObject(emptyMap()))
                                    }
                                },
                                onRemove = { askRemove = g },
                            )
                            RowDivider()
                        }
                    }
                }
            }
        }
    }
    val r = askRemove
    if (r != null) {
        ConfirmDialog(
            title = "Remove this download?",
            text = "Lantern stops following \"${r.title}\" and takes it out of the download client. Its files stay on the disk.",
            okLabel = "Remove",
            danger = true,
            onDismiss = { askRemove = null },
            onConfirm = {
                askRemove = null
                scope.serverCall<Any?>(ok = "Removed", after = { round += 1 }) { app.server.delete("/api/hub/grabs/${r.id}") }
            },
        )
    }
}

/** One download: its name and state, progress, its line, what went wrong, and Try again / Remove. */
@Composable
private fun GrabItem(g: Grab, now: Double, canChange: Boolean, onRetry: () -> Unit, onRemove: () -> Unit) {
    val (label, kind) = DownloadRules.stateBadge(g.state)
    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(g.title, style = MaterialTheme.typography.bodyMedium, maxLines = 2, modifier = Modifier.weight(1f))
            Badge(label, kind)
        }
        if (g.state == "downloading" || g.state == "paused") {
            LinearProgressIndicator(
                progress = { g.progress.toFloat() },
                modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
                color = MaterialTheme.palette.accent,
                trackColor = MaterialTheme.palette.border,
            )
        }
        Muted(DownloadRules.line(g))
        val problem = DownloadRules.problemLine(g, now)
        if (problem.isNotEmpty()) Text(problem, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.text, modifier = Modifier.padding(top = 4.dp))
        if (g.path.isNotEmpty() && !DownloadRules.busy(g)) Muted(g.path, Modifier.padding(top = 2.dp))
        if (canChange && g.state != "removed") {
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (g.state == "failed") SmallButton("Try again", iconRes = KitR.drawable.kit_ic_refresh, onClick = onRetry)
                SmallButton("Remove", iconRes = KitR.drawable.kit_ic_delete, danger = true, onClick = onRemove)
            }
        }
    }
}

/**
 * The Apps page: every app connected to Lantern (Sonarr, Radarr …) with what it handles, its folder and its
 * downloads by state; tapping one shows its downloads.
 *
 * @param app the app (its server).
 * @param onOpen opens an app's downloads (its id and name).
 * @param onBack back to the page below.
 */
@Composable
fun AppsPage(app: App, onOpen: (Long, String) -> Unit, onBack: () -> Unit) {
    var list by remember { mutableStateOf<List<HubApp>?>(null) }
    var error by remember { mutableStateOf("") }
    var round by remember { mutableIntStateOf(0) }
    LaunchedEffect(round) {
        error = ""
        try {
            list = withContext(Dispatchers.IO) { DownloadRules.appsOf(app.server.getObject("/api/hub/apps")) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "The apps couldn't be loaded"
        }
    }
    Column(Modifier.fillMaxSize()) {
        BackBar("Apps", onBack)
        val all = list
        when {
            all == null && error.isNotEmpty() -> ErrorBox("The apps couldn't be loaded: $error") { round += 1 }

            all == null -> Row(Modifier.fillMaxWidth().padding(Gutter), horizontalArrangement = Arrangement.Center) { Spinner(24.dp) }

            all.isEmpty() -> EmptyState(KitR.drawable.kit_ic_download, "No apps yet", "Apps show up here once they connect to Lantern with its address and API key.")

            else -> LazyColumn(Modifier.fillMaxSize()) {
                item { SectionHead(if (all.size == 1) "1 app" else "${all.size} apps", action = "Refresh", onAction = { round += 1 }) }
                items(all, key = { it.id }) { a ->
                    Column(Modifier.fillMaxWidth().clickable { onOpen(a.id, a.name) }.padding(horizontal = Gutter, vertical = 10.dp)) {
                        Text(a.name, style = MaterialTheme.typography.titleMedium)
                        Muted(DownloadRules.appLine(a))
                        if (a.importPath.isNotEmpty()) Muted("Folder: ${a.importPath}")
                        Muted("Last seen ${formatAgo(a.lastSeen)}")
                        if (a.callbackError.isNotEmpty()) {
                            Text(
                                "Lantern couldn't tell it about a finished download: ${a.callbackError}",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.palette.text,
                                modifier = Modifier.padding(top = 4.dp),
                            )
                        }
                    }
                    RowDivider()
                }
            }
        }
    }
}
