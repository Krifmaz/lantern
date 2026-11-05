package dev.krifmaz.lantern

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
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
import dev.krifmaz.kit.compose.core.formatAgo
import dev.krifmaz.kit.compose.settings.Muted
import dev.krifmaz.kit.compose.settings.serverCall
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
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import java.net.URLEncoder
import dev.krifmaz.kit.compose.R as KitR

/**
 * The Account health page: every tracker and Usenet account the account keeper watches, the ones at risk first,
 * with the site's numbers (ratio, upload, download, seed time, bonus, hit-and-runs) and every risk in words (a
 * sign-in or activity deadline, a login or cookie running out, the ratio, hit-and-runs). Admins can have Lantern
 * sign in now through the account's indexer, or say they signed in on the site today. Rules and numbers are typed
 * on the web page.
 *
 * @param app the app (its server).
 * @param admin whether the account may act (null while not known: the buttons show, and the server says no if it must).
 * @param onBack back to the page below.
 */
@Composable
fun AccountsPage(app: App, admin: Boolean?, onBack: () -> Unit) {
    val scope = rememberCoroutineScope()
    var list by remember { mutableStateOf<List<KeptAccount>?>(null) }
    var error by remember { mutableStateOf("") }
    var round by remember { mutableIntStateOf(0) }
    var busy by remember { mutableStateOf(setOf<String>()) }
    LaunchedEffect(round) {
        error = ""
        try {
            list = withContext(Dispatchers.IO) { KeeperRules.accountsOf(app.server.getObject("/api/keeper")) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "The accounts couldn't be loaded"
        }
    }
    // runs one of the account's actions, then loads the accounts again
    val act: (KeptAccount, String, () -> Any?) -> Unit = { a, ok, call ->
        busy = busy + a.id
        scope.serverCall<Any?>(
            ok = ok,
            onError = { msg ->
                busy = busy - a.id
                Toasts.show("${a.name}: $msg", "err")
            },
            after = {
                busy = busy - a.id
                round += 1
            },
            call = call,
        )
    }
    Column(Modifier.fillMaxSize()) {
        BackBar("Account health", onBack)
        val all = list
        when {
            all == null && error.isNotEmpty() -> ErrorBox("The accounts couldn't be loaded: $error") { round += 1 }

            all == null -> Row(Modifier.fillMaxWidth().padding(Gutter), horizontalArrangement = Arrangement.Center) { Spinner(24.dp) }

            all.isEmpty() -> EmptyState(KitR.drawable.kit_ic_warning, "No accounts kept yet", "Save your tracker and Usenet logins in the vault, and Lantern watches them here.")

            else -> LazyColumn(Modifier.fillMaxSize()) {
                item { SectionHead(KeeperRules.summaryLine(all), action = "Refresh", onAction = { round += 1 }) }
                items(all, key = { it.id }) { a ->
                    val id = URLEncoder.encode(a.id, "UTF-8")
                    AccountItem(
                        a,
                        canAct = admin != false,
                        busy = a.id in busy,
                        onSignIn = { act(a, "${a.name}: signed in") { app.server.post("/api/keeper/$id/sign-in", JsonObject(emptyMap())) } },
                        onSignedToday = { act(a, "${a.name}: noted, signed in today") { app.server.put("/api/keeper/$id/stats", KeeperRules.signedInTodayBody()) } },
                    )
                    RowDivider()
                }
            }
        }
    }
}

/** One account: its name and health, its numbers, its risks, the last sign-in, and the actions. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AccountItem(a: KeptAccount, canAct: Boolean, busy: Boolean, onSignIn: () -> Unit, onSignedToday: () -> Unit) {
    val health = KeeperRules.health(a)
    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(a.name, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            if (a.keepAlive) Badge("Kept alive", "info")
            Badge(health.label, health.kind)
        }
        Muted(KeeperRules.numbersLine(a))
        for (r in a.risks) {
            Row(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.Top) {
                Badge(if (r.danger) "Now" else "Soon", if (r.danger) "err" else "warn")
                Text(r.text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.text, modifier = Modifier.weight(1f))
            }
        }
        Muted("Last sign-in: ${formatAgo(a.signedInAt)}", Modifier.padding(top = 4.dp))
        if (a.keepAliveError.isNotEmpty()) {
            Text(
                "Lantern's last sign-in on its own didn't work: ${a.keepAliveError}",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.palette.text,
                modifier = Modifier.padding(top = 2.dp),
            )
        }
        if (canAct) {
            FlowRow(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                if (a.linked) SmallButton(if (busy) "Signing in…" else "Sign in now", iconRes = KitR.drawable.kit_ic_refresh, enabled = !busy, onClick = onSignIn)
                SmallButton("I signed in today", iconRes = KitR.drawable.kit_ic_check, enabled = !busy, onClick = onSignedToday)
            }
        }
    }
}
