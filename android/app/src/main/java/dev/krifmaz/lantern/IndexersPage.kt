package dev.krifmaz.lantern

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.krifmaz.kit.compose.settings.Muted
import dev.krifmaz.kit.compose.theme.palette
import dev.krifmaz.kit.compose.toasts.Toasts
import dev.krifmaz.kit.compose.widgets.BackBar
import dev.krifmaz.kit.compose.widgets.Badge
import dev.krifmaz.kit.compose.widgets.EmptyState
import dev.krifmaz.kit.compose.widgets.ErrorBox
import dev.krifmaz.kit.compose.widgets.Gutter
import dev.krifmaz.kit.compose.widgets.RowDivider
import dev.krifmaz.kit.compose.widgets.SectionHead
import dev.krifmaz.kit.compose.widgets.SmallButton
import dev.krifmaz.kit.compose.widgets.Spinner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import dev.krifmaz.kit.compose.R as KitR

/** How many days of numbers the Indexers page shows. */
private const val STATS_DAYS = 7

/** What the Indexers page has loaded: the indexers and their numbers. */
private data class IndexersData(
    /** The indexers, in the server's order. */
    val list: List<IndexerRow>,
    /** Their numbers over the last [STATS_DAYS] days, by id. */
    val stats: Map<Long, IndexerStats>,
)

/**
 * The Indexers page: every indexer with how it is doing (working, failing, on hold, off), what went wrong last and its
 * numbers over the last week. Admins can test one (Lantern signs in and searches it). Adding and changing indexers
 * stays on the web page, where the site's settings are.
 *
 * @param app the app (its server).
 * @param admin whether the account may test indexers (null while the server hasn't said: the button shows, and
 *   the server says no if it must).
 * @param onBack back to the page below.
 */
@Composable
fun IndexersPage(app: App, admin: Boolean?, onBack: () -> Unit) {
    val scope = rememberCoroutineScope()
    var data by remember { mutableStateOf<IndexersData?>(null) }
    var error by remember { mutableStateOf("") }
    var round by remember { mutableIntStateOf(0) }
    var testing by remember { mutableStateOf(setOf<Long>()) }
    LaunchedEffect(round) {
        error = ""
        try {
            data = withContext(Dispatchers.IO) {
                val list = IndexerRules.indexersOf(app.server.getObject("/api/indexers"))
                val stats = IndexerRules.statsOf(app.server.getObject("/api/indexers/stats?days=$STATS_DAYS"))
                IndexersData(list, stats)
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "The indexers couldn't be loaded"
        }
    }
    Column(Modifier.fillMaxSize()) {
        BackBar("Indexers", onBack)
        val d = data
        when {
            d == null && error.isNotEmpty() -> ErrorBox("The indexers couldn't be loaded: $error") { round += 1 }

            d == null -> Row(Modifier.fillMaxWidth().padding(Gutter), horizontalArrangement = Arrangement.Center) { Spinner(24.dp) }

            d.list.isEmpty() -> EmptyState(KitR.drawable.kit_ic_search, "No indexers yet", "Add your sites on Lantern's web page under Indexers.")

            else -> {
                val now = System.currentTimeMillis() / 1000.0
                LazyColumn(Modifier.fillMaxSize()) {
                    item {
                        SectionHead(
                            IndexerRules.summaryLine(IndexerRules.summary(d.list, now)),
                            subtitle = "Numbers from the last $STATS_DAYS days",
                            action = "Refresh",
                            onAction = { round += 1 },
                        )
                    }
                    items(d.list, key = { it.id }) { ix ->
                        IndexerItem(
                            ix,
                            d.stats[ix.id],
                            now,
                            showTest = admin != false,
                            testing = ix.id in testing,
                            onTest = {
                                testing = testing + ix.id
                                scope.launch {
                                    try {
                                        val o = withContext(Dispatchers.IO) { app.server.post("/api/indexers/${ix.id}/test", JsonObject(emptyMap())) } as? JsonObject
                                        val r = IndexerRules.testOf(o)
                                        Toasts.show("${ix.name}: ${r.message}", if (r.ok) "ok" else "err")
                                        round += 1
                                    } catch (e: CancellationException) {
                                        throw e
                                    } catch (e: Exception) {
                                        Toasts.show("${ix.name} couldn't be tested: ${e.message ?: e.javaClass.simpleName}", "err")
                                    } finally {
                                        testing = testing - ix.id
                                    }
                                }
                            },
                        )
                        RowDivider()
                    }
                }
            }
        }
    }
}

/** One indexer: its name and health badge, its kind, what is wrong and its numbers, and Test. */
@Composable
private fun IndexerItem(ix: IndexerRow, stats: IndexerStats?, now: Double, showTest: Boolean, testing: Boolean, onTest: () -> Unit) {
    val health = IndexerRules.health(ix, now)
    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(ix.name, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            Badge(health.label, health.kind)
        }
        Muted(listOf(ix.protocol.replaceFirstChar { it.uppercase() }, ix.privacy).filter { it.isNotEmpty() }.joinToString(" · "))
        val line = IndexerRules.healthLine(ix, now)
        if (line.isNotEmpty()) Text(line, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.text, modifier = Modifier.padding(top = 4.dp))
        Muted(IndexerRules.statsLine(stats, STATS_DAYS), Modifier.padding(top = 4.dp))
        if (showTest) {
            Row(Modifier.padding(top = 8.dp)) {
                SmallButton(if (testing) "Testing…" else "Test", iconRes = KitR.drawable.kit_ic_refresh, enabled = !testing, onClick = onTest)
            }
        }
    }
}
