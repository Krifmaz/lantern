package dev.krifmaz.lantern

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.PersistableBundle
import dev.krifmaz.kit.compose.toasts.Toasts

// What the pages ask of the phone itself: opening a link in the browser and putting text on the clipboard.

/** Opens [url] in the phone's browser; says so when nothing can open it. */
fun openLink(context: Context, url: String) {
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (_: ActivityNotFoundException) {
        Toasts.show("No app on this phone can open $url", "err")
    }
}

/**
 * Puts [text] on the clipboard as [label], and says so. A [secret] is marked sensitive, so Android doesn't show it
 * in its copy preview (Android 13 and newer).
 */
fun copyText(context: Context, label: String, text: String, secret: Boolean = false) {
    val cm = context.getSystemService(ClipboardManager::class.java)
    if (cm == null) {
        Toasts.show("The clipboard isn't available on this phone", "err")
        return
    }
    val clip = ClipData.newPlainText(label, text)
    if (secret && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        clip.description.extras = PersistableBundle().apply { putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true) }
    }
    cm.setPrimaryClip(clip)
    Toasts.show("$label copied", "ok")
}
