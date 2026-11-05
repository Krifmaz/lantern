package dev.krifmaz.lantern

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.unit.dp
import dev.krifmaz.kit.compose.KitApp
import dev.krifmaz.kit.compose.bugs.BugReportsEntry
import dev.krifmaz.kit.compose.bugs.BugReportsHost
import dev.krifmaz.kit.compose.bugs.BugRules
import dev.krifmaz.kit.compose.bugs.CrashLog
import dev.krifmaz.kit.compose.bugs.CrashLogReport
import dev.krifmaz.kit.compose.bugs.NewBugReportSheet
import dev.krifmaz.kit.compose.bugs.saveExport
import dev.krifmaz.kit.compose.dialogs.ConfirmDialog
import dev.krifmaz.kit.compose.notices.NoticeBanners
import dev.krifmaz.kit.compose.notices.rememberNotices
import dev.krifmaz.kit.compose.offline.OfflineBanner
import dev.krifmaz.kit.compose.offline.probeHealth
import dev.krifmaz.kit.compose.settings.Muted
import dev.krifmaz.kit.compose.theme.palette
import dev.krifmaz.kit.compose.toasts.Toasts
import dev.krifmaz.kit.compose.update.UpdatesSection
import dev.krifmaz.kit.compose.whatsnew.WhatsNewOnStart
import dev.krifmaz.kit.compose.widgets.Gutter
import dev.krifmaz.kit.compose.widgets.RowDivider
import dev.krifmaz.kit.compose.widgets.SectionHead
import dev.krifmaz.kit.compose.widgets.SmallButton
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import dev.krifmaz.kit.compose.R as KitR

/**
 * The first page: the offline banner when the server can't be reached, What's new once after an update, the
 * server's notices (admins), how the indexers are doing with the entries to Indexers and Search, the settings
 * entries, bug reports, the crash log and updates. The list's resource id ("home-list") is what the benchmarks scroll.
 *
 * @param app the app (its server, settings and calls).
 * @param base the server's address.
 * @param bugs the bug report screens' host.
 * @param open opens another page.
 * @param onChangeServer forgets the server, back to [ConnectPage].
 */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
fun HomePage(app: App, base: String, bugs: BugReportsHost, open: (Page) -> Unit, onChangeServer: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // who is looking: null until the server says (or when it can't); the admin pages show unless it says no
    var me by remember { mutableStateOf<Me?>(null) }
    // how the indexers are doing, in one line ("" until the server says)
    var health by remember { mutableStateOf("") }
    var askSignOut by remember { mutableStateOf(false) }
    var newReport by remember { mutableStateOf(false) }
    // offline mode: one /api/health check at start and on every Retry
    var offline by remember { mutableStateOf(false) }
    var checking by remember { mutableStateOf(false) }
    var reason by remember { mutableStateOf("") }
    var round by remember { mutableIntStateOf(0) }
    LaunchedEffect(base, round) {
        checking = true
        val why = withContext(Dispatchers.IO) { probeHealth(app.server.http, base, phoneOnline(context), waitSeconds = 8) }
        offline = why != null
        reason = why.orEmpty()
        checking = false
        if (!offline) {
            try {
                me = withContext(Dispatchers.IO) { app.server.me() }
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // the last answer stays (null: the admin pages show, and the server says no there)
            }
            health = try {
                val list = withContext(Dispatchers.IO) { IndexerRules.indexersOf(app.server.getObject("/api/indexers")) }
                IndexerRules.summaryLine(IndexerRules.summary(list, System.currentTimeMillis() / 1000.0))
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                "The indexers couldn't be loaded: ${e.message ?: e.javaClass.simpleName}"
            }
        }
    }
    // the server's notices (an indexer that stopped working, the data folder …), as banners for admins
    val notices = rememberNotices(app.server.kit, admin = me?.admin == true)
    // What's new: once after an update, when the server says there is something new for this account
    WhatsNewOnStart(app.changelog, onFullChangelog = { open(Page.Changelog) })

    Column(Modifier.fillMaxSize()) {
        OfflineBanner(
            active = offline,
            trying = checking,
            title = "Offline",
            line = "Can't reach the server · tap for details",
            message = "Offline – ${KitApp.name} can't reach its server at $base.",
            reason = reason,
            status = "",
            onRetry = { round += 1 },
        )
        Column(
            Modifier
                .fillMaxSize()
                .semantics { testTagsAsResourceId = true }
                .testTag("home-list")
                .verticalScroll(rememberScrollState()),
        ) {
            SectionHead(KitApp.name, subtitle = base.substringAfter("://"), action = "Change server", onAction = onChangeServer)

            NoticeBanners(notices, admin = me?.admin == true, modifier = Modifier.padding(horizontal = Gutter))

            SectionHead("Indexers", subtitle = health)
            EntryRow("Indexers", "How every site is doing, what went wrong and its numbers") { open(Page.Indexers(me?.admin)) }
            EntryRow("Search", "Search every indexer at once") { open(Page.Search) }

            SectionHead("Settings")
            EntryRow("What's new", "Every release's notes") { open(Page.Changelog) }
            if (me?.admin != false) {
                EntryRow("Server settings", "The server's settings, as the web app has them") { open(Page.ServerSettings) }
                EntryRow("Diagnostics", "Checks of the server and the phone app's crash reports") { open(Page.Diagnostics) }
            }
            EntryRow("Sign out", "Sign in again, as someone else or after a password change") { askSignOut = true }
            BugReportsEntry(bugs)
            Row(Modifier.padding(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SmallButton("New report", iconRes = KitR.drawable.kit_ic_add) { newReport = true }
            }
            RowDivider(Modifier.padding(top = 12.dp))
            CrashReports(app)
            RowDivider()
            UpdatesSection()
            Box(Modifier.height(32.dp))
        }
    }

    if (askSignOut) {
        ConfirmDialog(
            title = "Sign out?",
            text = "This phone forgets its sign-in. You'll need your password to sign in again.",
            okLabel = "Sign out",
            danger = true,
            onDismiss = { askSignOut = false },
            onConfirm = { scope.launch { withContext(Dispatchers.IO) { app.server.logout() } } },
        )
    }
    if (newReport) {
        NewBugReportSheet(bugs, page = "Home", onDismiss = { newReport = false })
    }
}

/** A row that opens a page: its name and a line about it (the look of the kit's Bug reports entry). */
@Composable
private fun EntryRow(title: String, line: String, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 10.dp)) {
        Text(title)
        Text(line, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted)
    }
}

/**
 * The crash reports this phone keeps until the server has them (admins read them under Diagnostics → App crashes),
 * Send crash log (a bug report of the kind Crash log) and Save as .txt (straight into Downloads, with Share on the
 * toast).
 */
@Composable
private fun CrashReports(app: App) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var sending by remember { mutableStateOf(false) }
    var version by remember { mutableIntStateOf(0) }
    // the count lists a folder: read off the main thread
    var waiting by remember { mutableIntStateOf(0) }
    LaunchedEffect(version) {
        if (version > 0) delay(2000)
        waiting = withContext(Dispatchers.IO) { CrashLog.pendingCount() }
    }
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
        Text("App crashes", style = MaterialTheme.typography.titleMedium)
        Muted("If ${KitApp.name} closed by itself, send the crash log: it goes to the admin's Bug reports under Crash log.")
        Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SmallButton(if (sending) "Sending…" else "Send crash log", iconRes = KitR.drawable.kit_ic_bug, enabled = !sending) {
                sending = true
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) { CrashLogReport.send(app, app.bugReports) }
                        Toasts.show("Crash log sent to the admin")
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(BugRules.errorWords(e), "err")
                    } finally {
                        sending = false
                    }
                }
            }
            SmallButton("Save as .txt", iconRes = KitR.drawable.kit_ic_download) {
                scope.launch {
                    try {
                        saveExport(context, withContext(Dispatchers.IO) { CrashLog.exportText(context) })
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show("The crash log couldn't be saved: ${e.message ?: e.javaClass.simpleName}", "err")
                    }
                }
            }
        }
        if (waiting > 0) {
            Row(Modifier.padding(top = 6.dp)) {
                Muted("Waiting to send: $waiting", Modifier.weight(1f), color = MaterialTheme.palette.text)
                SmallButton("Send now", iconRes = KitR.drawable.kit_ic_refresh) {
                    CrashLog.uploadPending(app, app)
                    version += 1
                }
            }
        }
    }
}
