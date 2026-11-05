package dev.krifmaz.lantern

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import dev.krifmaz.kit.compose.KitApp
import dev.krifmaz.kit.compose.details.CopyIconButton
import dev.krifmaz.kit.compose.theme.KitStatusColors
import dev.krifmaz.kit.compose.theme.palette
import dev.krifmaz.kit.compose.widgets.Gutter
import dev.krifmaz.kit.compose.widgets.KitButton
import dev.krifmaz.kit.compose.widgets.KitField
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The first start (and Change server): the server's address, suggested from the build ([BuildConfig.DEFAULT_SERVER])
 * or the last one used. Continue checks the address ([ServerApi.probe]) and keeps it;
 * [SignInPage] comes next. The field and button carry resource ids ("connect-address", "connect-continue") for the
 * benchmarks.
 */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
fun ConnectPage(app: App, onConnected: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var address by remember { mutableStateOf(app.settings.last.ifEmpty { BuildConfig.DEFAULT_SERVER }) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }

    fun go() {
        if (busy) return
        busy = true
        error = ""
        scope.launch {
            try {
                val base = withContext(Dispatchers.IO) { app.server.probe(address) }
                app.settings.connect(base)
                onConnected()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message ?: ServerAddress.UNREACHABLE
            } finally {
                busy = false
            }
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .semantics { testTagsAsResourceId = true }
            .verticalScroll(rememberScrollState())
            .padding(horizontal = Gutter, vertical = 48.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(KitApp.name, style = MaterialTheme.typography.displaySmall)
        Text("Connect to your server", color = p.muted)
        KitField(
            address,
            {
                address = it
                error = ""
            },
            modifier = Modifier.fillMaxWidth().testTag("connect-address"),
            label = "Server address",
            placeholder = "server.example.com",
            keyboardType = KeyboardType.Uri,
            imeAction = ImeAction.Go,
            onAction = ::go,
        )
        if (error.isNotEmpty()) ErrorLine(error)
        KitButton(
            if (busy) "Connecting…" else "Continue",
            modifier = Modifier.fillMaxWidth().testTag("connect-continue"),
            primary = true,
            enabled = !busy && address.isNotBlank(),
            onClick = ::go,
        )
    }
}

/** An error under a form, in full and selectable, with Copy beside it (the kit's copyable errors). */
@Composable
fun ErrorLine(message: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        SelectionContainer(Modifier.weight(1f)) { Text(message, color = KitStatusColors.Err) }
        CopyIconButton(message)
    }
}
