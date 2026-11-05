package dev.krifmaz.lantern

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.krifmaz.kit.compose.perf.KitPerf
import dev.krifmaz.kit.compose.theme.KitTheme
import dev.krifmaz.kit.compose.toasts.ToastHost
import dev.krifmaz.kit.compose.update.Updater

/** The app's one activity: the kit's default theme, the app's pages ([AppShell]) and the toasts above them. */
class MainActivity : ComponentActivity() {
    /** Draws the app and looks for a new version once a day. */
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val app = application as App
        setContent {
            KitTheme(theme = KitTheme.Encore) {
                Box(Modifier.fillMaxSize().safeDrawingPadding()) {
                    AppShell(app)
                    ToastHost(bottomPadding = 16.dp)
                }
            }
        }
        Updater.checkAtStart(this)
        KitPerf.jankStatsInDebug(this, BuildConfig.DEBUG)
    }

    /** Back from Android's "Install unknown apps" page: the updater goes on with the install when it may now. */
    override fun onResume() {
        super.onResume()
        Updater.onResume(this)
    }
}
