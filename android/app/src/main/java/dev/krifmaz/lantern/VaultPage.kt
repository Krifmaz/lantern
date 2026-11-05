package dev.krifmaz.lantern

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.krifmaz.kit.compose.settings.Mono
import dev.krifmaz.kit.compose.settings.Muted
import dev.krifmaz.kit.compose.settings.SecureWindow
import dev.krifmaz.kit.compose.theme.palette
import dev.krifmaz.kit.compose.toasts.Toasts
import dev.krifmaz.kit.compose.widgets.BackBar
import dev.krifmaz.kit.compose.widgets.Badge
import dev.krifmaz.kit.compose.widgets.EmptyState
import dev.krifmaz.kit.compose.widgets.ErrorBox
import dev.krifmaz.kit.compose.widgets.Gutter
import dev.krifmaz.kit.compose.widgets.KitField
import dev.krifmaz.kit.compose.widgets.RowDivider
import dev.krifmaz.kit.compose.widgets.SectionHead
import dev.krifmaz.kit.compose.widgets.SmallButton
import dev.krifmaz.kit.compose.widgets.Spinner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import java.net.URLEncoder
import dev.krifmaz.kit.compose.R as KitR

/** How long revealed values stay on the screen before they are hidden again, in seconds. */
private const val SHOW_SECONDS = 120

/** What the Vault page has loaded: whether the vault is open, its fields and its accounts. */
private data class VaultData(
    /** Whether it is open, why not, and its fields. */
    val status: VaultStatus,
    /** The accounts (empty while it is locked). */
    val items: List<VaultItem>,
)

/**
 * The Vault page: the tracker and Usenet accounts saved in Lantern's vault, by kind. An account shows its plain
 * values (username, server …) and which secrets are saved; the owner sees the secrets and the 2FA code after typing
 * their password again, for two minutes, with screenshots blocked meanwhile. Every value can be copied. Adding,
 * changing and backing up accounts stays on the web page.
 *
 * @param app the app (its server).
 * @param owner whether the account is the server's owner, who alone may see secrets (null while not known: the
 *   password field shows, and the server says no if it must).
 * @param onBack back to the page below.
 */
@Composable
fun VaultPage(app: App, owner: Boolean?, onBack: () -> Unit) {
    var data by remember { mutableStateOf<VaultData?>(null) }
    var error by remember { mutableStateOf("") }
    var round by remember { mutableIntStateOf(0) }
    var openId by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(round) {
        error = ""
        try {
            data = withContext(Dispatchers.IO) {
                val status = VaultRules.statusOf(app.server.getObject("/api/vault"))
                val items = if (status.open) VaultRules.itemsOf(app.server.getObject("/api/vault/items")) else emptyList()
                VaultData(status, items)
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "The vault couldn't be loaded"
        }
    }
    val d = data
    val chosen = d?.items?.firstOrNull { it.id == openId }
    if (chosen != null && d != null) {
        // Back closes the account first, then the page
        BackHandler { openId = null }
        VaultItemView(app, chosen, d.status.fields, owner, onBack = { openId = null })
        return
    }
    Column(Modifier.fillMaxSize()) {
        BackBar("Vault", onBack)
        when {
            d == null && error.isNotEmpty() -> ErrorBox("The vault couldn't be loaded: $error") { round += 1 }

            d == null -> Row(Modifier.fillMaxWidth().padding(Gutter), horizontalArrangement = Arrangement.Center) { Spinner(24.dp) }

            !d.status.open -> ErrorBox(VaultRules.lockedWords(d.status)) { round += 1 }

            d.items.isEmpty() -> EmptyState(KitR.drawable.kit_ic_file, "No accounts in the vault yet", "Add your tracker and Usenet logins on Lantern's web page under Vault.")

            else -> LazyColumn(Modifier.fillMaxSize()) {
                item {
                    SectionHead(
                        if (d.items.size == 1) "1 account" else "${d.items.size} accounts",
                        subtitle = "Saved encrypted on your Lantern server",
                        action = "Refresh",
                        onAction = { round += 1 },
                    )
                }
                items(d.items, key = { it.id }) { item ->
                    Column(Modifier.fillMaxWidth().clickable { openId = item.id }.padding(horizontal = Gutter, vertical = 10.dp)) {
                        Text(item.name, style = MaterialTheme.typography.titleMedium)
                        Muted(VaultRules.line(item, d.status.fields))
                        if (item.site.isNotEmpty()) Muted(item.site.substringAfter("://"))
                    }
                    RowDivider()
                }
            }
        }
    }
}

/**
 * One account: its values with Copy, and for the owner the password field that shows the secrets and the 2FA code.
 * Revealed values are forgotten when the account is closed and after [SHOW_SECONDS].
 */
@Composable
private fun VaultItemView(app: App, item: VaultItem, fields: List<VaultField>, owner: Boolean?, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var revealed by remember(item.id) { mutableStateOf<RevealedItem?>(null) }
    var password by remember(item.id) { mutableStateOf("") }
    var busy by remember(item.id) { mutableStateOf(false) }
    var error by remember(item.id) { mutableStateOf("") }
    var left by remember(item.id) { mutableIntStateOf(0) }
    var codeLeft by remember(item.id) { mutableIntStateOf(0) }
    SecureWindow(revealed != null)
    LaunchedEffect(revealed) {
        val r = revealed ?: return@LaunchedEffect
        left = SHOW_SECONDS
        codeLeft = r.totp?.secondsLeft ?: 0
        while (left > 0) {
            delay(1000)
            left -= 1
            if (codeLeft > 0) codeLeft -= 1
        }
        revealed = null
    }
    val reveal: () -> Unit = reveal@{
        if (password.isEmpty()) {
            error = VaultRules.NO_PASSWORD
            return@reveal
        }
        busy = true
        error = ""
        scope.launch {
            try {
                val id = URLEncoder.encode(item.id, "UTF-8")
                val o = withContext(Dispatchers.IO) { app.server.post("/api/vault/items/$id/reveal", VaultRules.revealBody(password)) } as? JsonObject
                revealed = VaultRules.revealedOf(o)
                password = ""
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message ?: "The secrets couldn't be shown"
            } finally {
                busy = false
            }
        }
    }
    Column(Modifier.fillMaxSize()) {
        BackBar(item.name, onBack)
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            Column(Modifier.padding(horizontal = Gutter, vertical = 8.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Badge(item.kindLabel)
                    if (revealed != null) Badge("Shown for $left s", "warn")
                }
                if (item.site.isNotEmpty()) {
                    Row(Modifier.padding(top = 8.dp)) {
                        SmallButton(item.site.substringAfter("://"), iconRes = KitR.drawable.kit_ic_open_in_new) { openLink(context, item.site) }
                    }
                }
            }
            for ((label, value, secret) in VaultRules.rows(item, fields, revealed)) {
                val hidden = secret && revealed == null
                Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Muted(label)
                        Mono(value, color = if (hidden) MaterialTheme.palette.muted else MaterialTheme.palette.text)
                    }
                    if (!hidden) SmallButton("Copy", iconRes = KitR.drawable.kit_ic_content_copy) { copyText(context, label, value, secret) }
                }
                RowDivider()
            }
            val code = revealed?.totp
            if (code != null) {
                Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Muted(if (codeLeft > 0) "2FA code · good for $codeLeft s" else "2FA code · used up, show the secrets again for a new one")
                        Text(VaultRules.codeWords(code.code), style = MaterialTheme.typography.headlineSmall)
                    }
                    if (codeLeft > 0) SmallButton("Copy", iconRes = KitR.drawable.kit_ic_content_copy) { copyText(context, "2FA code", code.code, secret = true) }
                }
                RowDivider()
            }
            if (item.secrets.isNotEmpty() && owner != false) {
                Column(Modifier.padding(Gutter), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (revealed == null) {
                        Muted("To see the saved secrets, type the password you sign in to Lantern with.")
                        KitField(password, { password = it }, Modifier.fillMaxWidth(), label = "Your password", password = true, onAction = reveal)
                        if (error.isNotEmpty()) Text(error, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.text)
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            SmallButton(if (busy) "Checking…" else "Show secrets", primary = true, enabled = !busy, onClick = reveal)
                            if (busy) Spinner()
                        }
                    } else {
                        SmallButton("Hide now") {
                            revealed = null
                            Toasts.show("Hidden")
                        }
                    }
                }
            } else if (item.secrets.isNotEmpty()) {
                Muted("Only the server's owner can see the saved secrets.", Modifier.padding(Gutter))
            }
        }
    }
}
