package dev.krifmaz.lantern

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.krifmaz.kit.compose.settings.AppCrashesSection
import dev.krifmaz.kit.compose.settings.DiagnosticsRules
import dev.krifmaz.kit.compose.settings.DiagnosticsScreen
import dev.krifmaz.kit.compose.settings.Muted
import dev.krifmaz.kit.compose.settings.SchemaApi
import dev.krifmaz.kit.compose.settings.SchemaFormState
import dev.krifmaz.kit.compose.settings.SchemaPage
import dev.krifmaz.kit.compose.settings.SettingsPage
import dev.krifmaz.kit.compose.settings.loadState
import dev.krifmaz.kit.compose.widgets.ChoiceRow
import dev.krifmaz.kit.compose.widgets.ErrorBox
import dev.krifmaz.kit.compose.widgets.Gutter
import dev.krifmaz.kit.compose.widgets.Spinner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Server settings: the server's settings schema (`GET /api/settings/schema`) drawn as the web's cards, one
 * page of them at a time (a choice row picks the page), with each card's Test and Save. Admins only; the server's
 * owner may also show saved secrets.
 */
@Composable
fun ServerSettingsPage(app: App, onBack: () -> Unit) {
    val schemaApi = remember(app) { SchemaApi(app.server.kit, app.server.http) }
    var state by remember { mutableStateOf<SchemaFormState?>(null) }
    var error by remember { mutableStateOf("") }
    var round by remember { mutableIntStateOf(0) }
    var page by remember { mutableIntStateOf(0) }
    LaunchedEffect(round) {
        error = ""
        try {
            state =
                withContext(Dispatchers.IO) {
                    val owner = runCatching { app.server.me().owner }.getOrDefault(false)
                    schemaApi.loadState(canReveal = owner)
                }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    SettingsPage("Server settings", onBack, gutter = false) {
        val s = state
        when {
            error.isNotEmpty() -> ErrorBox(error) { round += 1 }

            s == null -> Box(Modifier.fillMaxWidth().padding(32.dp), contentAlignment = Alignment.Center) { Spinner() }

            s.schema.sections.isEmpty() -> Muted("This server has no settings to change here.", Modifier.padding(horizontal = Gutter))

            else -> {
                val sections = s.schema.sections
                if (sections.size > 1) ChoiceRow(sections.map { it.title }, page.coerceAtMost(sections.lastIndex), { page = it })
                SchemaPage(s, schemaApi, sections[page.coerceAtMost(sections.lastIndex)], Modifier.padding(horizontal = Gutter, vertical = 12.dp))
            }
        }
    }
}

/**
 * Diagnostics: the server's checks (`GET /api/diagnostics`) with Copy report and Run checks, then the crash
 * reports the phone app sent (App crashes, with Clear). Admins only.
 */
@Composable
fun DiagnosticsPage(app: App, onBack: () -> Unit) {
    DiagnosticsScreen(
        title = "Diagnostics",
        subtitle = "What the server checks, with the raw answers. Copy report puts it all on the clipboard.",
        onBack = onBack,
        load = { app.server.getObject("/api/diagnostics") },
        report = { DiagnosticsRules.report(it) },
        bottom = {
            AppCrashesSection(
                load = { app.server.getObject("/api/crash-reports") },
                clear = { app.server.delete("/api/crash-reports") },
            )
        },
    )
}
