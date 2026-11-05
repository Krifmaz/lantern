package dev.krifmaz.lantern

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import dev.krifmaz.kit.compose.KitApp
import dev.krifmaz.kit.compose.bugs.CrashLog
import dev.krifmaz.kit.compose.settings.TextLink
import dev.krifmaz.kit.compose.theme.palette
import dev.krifmaz.kit.compose.widgets.Gutter
import dev.krifmaz.kit.compose.widgets.KitButton
import dev.krifmaz.kit.compose.widgets.KitField
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import dev.krifmaz.kit.compose.R as KitR

/**
 * Signing in to the connected server, with the kit's sign-in rules: Username and
 * Password, then the authenticator code when the account has two-step sign-in. The server's answer (for example
 * "Invalid username or password") shows in full with Copy. The app token it gives is kept ([ServerApi.login]);
 * the app comes back here whenever the server stops taking it.
 *
 * @param app the app (its server).
 * @param base the server's address, under the title.
 * @param onBack the back arrow: choose another server.
 */
@Composable
fun SignInPage(app: App, base: String, onBack: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var username by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var needsCode by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }

    fun submit() {
        if (busy || username.isBlank() || password.isEmpty() || (needsCode && code.length < 6)) return
        busy = true
        error = ""
        scope.launch {
            try {
                when (val r = withContext(Dispatchers.IO) { app.server.login(username, password, if (needsCode) code else null) }) {
                    // the shell shows the app now; crash reports kept from before can go
                    is LoginResult.Success -> CrashLog.uploadPending(app, app)

                    is LoginResult.NeedsCode -> {
                        if (needsCode) error = r.message
                        needsCode = true
                    }

                    is LoginResult.Failed -> error = r.message
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message ?: SignInRules.FAILED
            } finally {
                busy = false
            }
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = Gutter, vertical = 48.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(painterResource(KitR.drawable.kit_ic_arrow_back), contentDescription = "Back") }
            Column {
                Text(SignInRules.title(KitApp.name), style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
                Text(base, style = MaterialTheme.typography.bodySmall, color = p.muted)
            }
        }
        KitField(username, { username = it }, modifier = Modifier.fillMaxWidth(), label = "Username", enabled = !needsCode, imeAction = ImeAction.Next)
        KitField(
            password,
            { password = it },
            modifier = Modifier.fillMaxWidth(),
            label = "Password",
            enabled = !needsCode,
            password = true,
            keyboardType = KeyboardType.Password,
            imeAction = ImeAction.Done,
            onAction = ::submit,
        )
        if (needsCode) {
            Text(SignInRules.CODE_PROMPT, color = p.muted)
            KitField(
                code,
                { v -> code = v.filter { it.isDigit() }.take(8) },
                modifier = Modifier.fillMaxWidth(),
                label = "Code",
                keyboardType = KeyboardType.NumberPassword,
                imeAction = ImeAction.Done,
                onAction = ::submit,
            )
        }
        if (error.isNotEmpty()) ErrorLine(error)
        KitButton(
            if (busy) "Signing in…" else "Sign in",
            modifier = Modifier.fillMaxWidth(),
            primary = true,
            enabled = !busy && username.isNotBlank() && password.isNotEmpty() && (!needsCode || code.length >= 6),
            onClick = ::submit,
        )
        if (needsCode) {
            TextLink("Use a different account") {
                needsCode = false
                code = ""
                error = ""
            }
        }
    }
}
