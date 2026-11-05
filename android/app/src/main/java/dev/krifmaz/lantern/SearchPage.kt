package dev.krifmaz.lantern

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import dev.krifmaz.kit.compose.settings.Muted
import dev.krifmaz.kit.compose.theme.palette
import dev.krifmaz.kit.compose.toasts.Toasts
import dev.krifmaz.kit.compose.widgets.BackBar
import dev.krifmaz.kit.compose.widgets.Badge
import dev.krifmaz.kit.compose.widgets.ChoiceRow
import dev.krifmaz.kit.compose.widgets.EmptyState
import dev.krifmaz.kit.compose.widgets.Gutter
import dev.krifmaz.kit.compose.widgets.KitField
import dev.krifmaz.kit.compose.widgets.RowDivider
import dev.krifmaz.kit.compose.widgets.SmallButton
import dev.krifmaz.kit.compose.widgets.Spinner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import dev.krifmaz.kit.compose.R as KitR

/**
 * The Search page: one search through every indexer at once (the same search as the web page's), by kind. Each
 * result shows its size, seeders, age and indexer; tapping it opens its site page and copies its download or magnet
 * link, to hand to a download client.
 *
 * @param app the app (its server).
 * @param onBack back to the page below.
 */
@Composable
fun SearchPage(app: App, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var words by rememberSaveable { mutableStateOf("") }
    var kind by rememberSaveable { mutableIntStateOf(0) }
    var answer by remember { mutableStateOf<SearchAnswer?>(null) }
    var error by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var open by remember { mutableIntStateOf(-1) }
    var job by remember { mutableStateOf<Job?>(null) }
    val run: () -> Unit = run@{
        val path = SearchRules.path(words, SearchKind.entries[kind])
        if (path == null) {
            Toasts.show(SearchRules.EMPTY)
            return@run
        }
        job?.cancel()
        busy = true
        error = ""
        open = -1
        job = scope.launch {
            try {
                answer = SearchRules.answerOf(withContext(Dispatchers.IO) { app.server.getObject(path) })
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = "The search didn't work: ${e.message ?: e.javaClass.simpleName}"
                Toasts.show(error, "err")
            } finally {
                busy = false
            }
        }
    }
    Column(Modifier.fillMaxSize()) {
        BackBar("Search", onBack)
        Column(Modifier.padding(horizontal = Gutter), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            KitField(
                words,
                { words = it },
                Modifier.fillMaxWidth(),
                placeholder = "A title, a show and season, an artist…",
                leadingIcon = KitR.drawable.kit_ic_search,
                imeAction = ImeAction.Search,
                onAction = run,
            )
            ChoiceRow(SearchKind.entries.map { it.label }, kind, { kind = it }, end = 0.dp)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                SmallButton(if (busy) "Searching…" else "Search", iconRes = KitR.drawable.kit_ic_search, primary = true, enabled = !busy, onClick = run)
                if (busy) Spinner()
            }
        }
        val a = answer
        when {
            a == null && error.isNotEmpty() -> Text(error, color = MaterialTheme.palette.text, modifier = Modifier.padding(Gutter))

            a == null -> EmptyState(KitR.drawable.kit_ic_search, "Search every indexer at once", "Lantern asks all your sites and shows what they found.")

            else -> LazyColumn(Modifier.fillMaxSize().padding(top = 8.dp)) {
                item {
                    Column(Modifier.padding(horizontal = Gutter, vertical = 6.dp)) {
                        Text(SearchRules.countLine(a), style = MaterialTheme.typography.titleMedium)
                        val failed = SearchRules.failedLine(a.failed)
                        if (failed.isNotEmpty()) Muted(failed)
                    }
                    RowDivider()
                }
                itemsIndexed(a.results) { i, r ->
                    ResultItem(r, expanded = open == i, onClick = { open = if (open == i) -1 else i }, context = context)
                    RowDivider()
                }
            }
        }
    }
}

/** One result: its name, its line and flags; open, the site page and the links to copy. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ResultItem(r: SearchResult, expanded: Boolean, onClick: () -> Unit, context: Context) {
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = Gutter, vertical = 10.dp)) {
        Text(r.title, style = MaterialTheme.typography.bodyMedium, maxLines = if (expanded) Int.MAX_VALUE else 2)
        Muted(SearchRules.line(r))
        if (r.flags.isNotEmpty() || (expanded && r.categories.isNotEmpty())) {
            FlowRow(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                for (f in r.flags) Badge(SearchRules.flagWords(f), "ok")
                if (expanded) for (c in r.categories) Badge(c)
            }
        }
        if (expanded) {
            FlowRow(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                if (r.infoUrl.isNotEmpty()) SmallButton("Open page", iconRes = KitR.drawable.kit_ic_open_in_new) { openLink(context, r.infoUrl) }
                if (r.downloadUrl.isNotEmpty()) SmallButton("Copy download link", iconRes = KitR.drawable.kit_ic_content_copy) { copy(context, "Download link", r.downloadUrl) }
                if (r.magnetUrl.isNotEmpty()) SmallButton("Copy magnet", iconRes = KitR.drawable.kit_ic_content_copy) { copy(context, "Magnet link", r.magnetUrl) }
            }
        }
    }
}

/** Opens [url] in the phone's browser; says so when nothing can open it. */
private fun openLink(context: Context, url: String) {
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (_: ActivityNotFoundException) {
        Toasts.show("No app on this phone can open $url", "err")
    }
}

/** Puts [text] on the clipboard as [label], and says so. */
private fun copy(context: Context, label: String, text: String) {
    val cm = context.getSystemService(ClipboardManager::class.java)
    if (cm == null) {
        Toasts.show("The clipboard isn't available on this phone", "err")
        return
    }
    cm.setPrimaryClip(ClipData.newPlainText(label, text))
    Toasts.show("$label copied", "ok")
}
