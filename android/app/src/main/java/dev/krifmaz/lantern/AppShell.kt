package dev.krifmaz.lantern

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import coil3.ImageLoader
import dev.krifmaz.kit.compose.bugs.BugReportScreen
import dev.krifmaz.kit.compose.bugs.BugReportsApi
import dev.krifmaz.kit.compose.bugs.BugReportsHost
import dev.krifmaz.kit.compose.bugs.BugReportsScreen
import dev.krifmaz.kit.compose.whatsnew.ChangelogScreen
import dev.krifmaz.kit.compose.widgets.BackBar

/** The app's pages; [AppShell] keeps them as a stack, and Back goes to the one below. */
sealed interface Page {
    /** The first page: the server's notices, how the indexers are doing, and the entries to the other pages. */
    data object Home : Page

    /** Every indexer and how it is doing; [admin] may test them (null: not known yet). */
    data class Indexers(val admin: Boolean?) : Page

    /** One search through every indexer. */
    data object Search : Page

    /** What's new: every release's notes ([ChangelogScreen]). */
    data object Changelog : Page

    /** The list of bug reports; [from] names the page it was opened from (a new report sends it along). */
    data class BugReports(val from: String) : Page

    /** One bug report. */
    data class BugReport(val id: Long) : Page

    /** The server's settings, drawn from its schema (admins). */
    data object ServerSettings : Page

    /** The server's checks and the phone app's crash reports (admins). */
    data object Diagnostics : Page
}

/**
 * The app's root: [ConnectPage] until a server is chosen, [SignInPage] until someone is signed in (and again
 * whenever the server answers 401), then the page on top of the stack. Changing the server or signing out starts
 * again from [Page.Home].
 */
@Composable
fun AppShell(app: App) {
    val base by app.settings.base.collectAsState()
    val signedIn by app.settings.signedIn.collectAsState()
    var stack by remember { mutableStateOf(listOf<Page>(Page.Home)) }
    val open: (Page) -> Unit = { stack = stack + it }
    val back: () -> Unit = { if (stack.size > 1) stack = stack.dropLast(1) }
    if (base.isEmpty()) {
        ConnectPage(app, onConnected = { stack = listOf(Page.Home) })
        return
    }
    if (!signedIn) {
        SignInPage(app, base, onBack = { app.settings.forget() })
        // the next sign-in starts at Home
        LaunchedEffect(Unit) { stack = listOf(Page.Home) }
        return
    }
    val bugs = remember(app) { AppBugReports(app, open, back) }
    if (stack.size > 1) BackHandler(onBack = back)
    when (val page = stack.last()) {
        Page.Home -> HomePage(app, base, bugs, open, onChangeServer = { app.settings.forget() })

        is Page.Indexers -> IndexersPage(app, page.admin, onBack = back)

        Page.Search -> SearchPage(app, onBack = back)

        Page.Changelog ->
            Column(Modifier.fillMaxSize()) {
                BackBar("What's new", back)
                ChangelogScreen(app.changelog)
            }

        is Page.BugReports -> BugReportsScreen(bugs, from = page.from)

        is Page.BugReport -> BugReportScreen(bugs, page.id)

        Page.ServerSettings -> ServerSettingsPage(app, onBack = back)

        Page.Diagnostics -> DiagnosticsPage(app, onBack = back)
    }
}

/** What the kit's bug report screens need from the app: its calls, its image loader, who is looking, and the pages. */
private class AppBugReports(private val app: App, private val open: (Page) -> Unit, private val close: () -> Unit) : BugReportsHost {
    override val bugReports: BugReportsApi get() = app.bugReports

    override val imageLoader: ImageLoader get() = app.imageLoader

    override fun url(path: String): String = app.server.url(path)

    override fun isAdmin(): Boolean = app.server.me().admin

    override fun openList(from: String) = open(Page.BugReports(from))

    override fun openReport(id: Long) = open(Page.BugReport(id))

    override fun back() = close()
}
